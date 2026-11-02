//! Translate the supported Python lexical forms, then let the native regex parser
//! validate precedence, character ranges, repetition, captures, and engine limits.

mod casefold;

pub use casefold::expand_case;

use std::fmt::Write;

use regex_syntax::hir::HirKind;

// Python str.isspace(), also used by Unicode `re`, includes four C0 separators
// omitted from the Unicode White_Space property used by the Rust engine.
const SPACE: &str =
    r"\x09-\x0d\x1c-\x20\x85\xa0\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}";

/// The synthetic capture used to implement Python's terminal dollar before a final newline.
pub const FINAL_NEWLINE_ANCHOR: &str = r"(?P<imre.eol>\n)?\z";

/// Translate Python's ASCII character classes inside or outside a bracket expression.
pub fn ascii_class(escape: char) -> Option<&'static str> {
    Some(match escape {
        'd' => "[0-9]",
        'D' => "[^0-9]",
        'w' => "[A-Za-z0-9_]",
        'W' => "[^A-Za-z0-9_]",
        's' => r"[\x09-\x0d\x20]",
        'S' => r"[^\x09-\x0d\x20]",
        _ => return None,
    })
}

/// The engine expression and capture layout corresponding to a Python pattern.
pub struct FixedPattern {
    pub source: String,
    pub may_match_empty: bool,
    pub groups: Vec<Capture>,
}

/// A capture may be unnamed or absent on some successful paths.
pub struct Capture {
    pub name: Option<String>,
    pub required: bool,
}

/// Keep a zero-count group's capture slots when regex-syntax simplifies repetitions.
pub fn preserve_zero_repeat_groups(source: String) -> Result<String, String> {
    use regex_syntax::ast::{Ast, RepetitionKind::Range, RepetitionRange, parse::Parser};

    if !source.contains('{') || !source.contains('0') || !source.contains('(') {
        return Ok(source);
    }
    let ast = Parser::new().parse(&source).map_err(|e| e.to_string())?;
    let mut nodes = vec![&ast];
    let mut edits = Vec::new();
    while let Some(node) = nodes.pop() {
        match node {
            Ast::Repetition(rep) => {
                if matches!(*rep.ast, Ast::Group(_))
                    && matches!(
                        rep.op.kind,
                        Range(RepetitionRange::Exactly(0) | RepetitionRange::Bounded(0, 0))
                    )
                {
                    // The empty class makes this optional branch unreachable in both
                    // text and byte regexes, so its captures never participate.
                    let group = rep.ast.span();
                    edits.push((group.start.offset, group.start.offset, r"(?:[^\s\S]"));
                    edits.push((group.end.offset, rep.span.end.offset, ")?"));
                }
                nodes.push(&rep.ast);
            }
            Ast::Group(group) => nodes.push(&group.ast),
            Ast::Concat(concat) => nodes.extend(&concat.asts),
            Ast::Alternation(alt) => nodes.extend(&alt.asts),
            _ => {}
        }
    }
    if edits.is_empty() {
        return Ok(source);
    }
    edits.sort_unstable();
    let mut output = String::with_capacity(source.len());
    let mut copied = 0;
    for (start, end, replacement) in edits {
        output.push_str(&source[copied..start]);
        output.push_str(replacement);
        copied = end;
    }
    output.push_str(&source[copied..]);
    Ok(output)
}

/// Translate the supported Python pattern grammar without changing search semantics.
pub fn translate(pattern: &str, verbose: bool) -> Result<FixedPattern, String> {
    translate_with_flags(pattern, verbose, false, false)
}

/// Translate with the character classes selected by Python's ASCII flag.
pub fn translate_with_flags(
    pattern: &str,
    verbose: bool,
    ascii: bool,
    dotall: bool,
) -> Result<FixedPattern, String> {
    translate_for_runtime(pattern, verbose, ascii, dotall, true)
}

/// Character and case rules for Python pattern escapes.
#[derive(Clone, Copy, PartialEq)]
pub enum Alphabet {
    Unicode,
    UnicodeIgnoreCase,
    Ascii,
    AsciiIgnoreCase,
    Bytes,
}

impl Alphabet {
    fn ignores_case(self) -> bool {
        matches!(self, Self::UnicodeIgnoreCase | Self::AsciiIgnoreCase)
    }

    fn ascii(self) -> bool {
        matches!(self, Self::Ascii | Self::AsciiIgnoreCase | Self::Bytes)
    }

    fn scoped(self, flag: char, enable: bool) -> Result<Self, String> {
        if self == Self::Bytes {
            return Err("scoped text modes require a str pattern".into());
        }
        let (ascii, ignore_case) = match flag {
            'a' | 'u' if enable => (flag == 'a', self.ignores_case()),
            'i' => (self.ascii(), enable),
            _ => return Err("cannot turn off a text alphabet".into()),
        };
        Ok(match (ascii, ignore_case) {
            (false, false) => Self::Unicode,
            (false, true) => Self::UnicodeIgnoreCase,
            (true, false) => Self::Ascii,
            (true, true) => Self::AsciiIgnoreCase,
        })
    }
}

struct Escapes {
    alphabet: Alphabet,
    bracketed_word: bool,
}

/// Use the caller's regex flags to limit literal guards to sound text modes.
pub fn translate_for_runtime(
    pattern: &str,
    verbose: bool,
    ascii: bool,
    dotall: bool,
    literal_guards: bool,
) -> Result<FixedPattern, String> {
    translate_pattern(
        pattern,
        verbose,
        if ascii {
            Alphabet::Ascii
        } else {
            Alphabet::Unicode
        },
        dotall,
        literal_guards,
    )
}

/// Translate a boxed pattern with its byte and Unicode escape rules.
pub fn translate_pattern(
    pattern: &str,
    verbose: bool,
    alphabet: Alphabet,
    dotall: bool,
    literal_guards: bool,
) -> Result<FixedPattern, String> {
    translate_pattern_with_line_mode(pattern, verbose, alphabet, dotall, literal_guards, false)
}

/// Translate a boxed pattern with the matcher's line mode for zero-width anchors.
pub fn translate_pattern_with_line_mode(
    pattern: &str,
    verbose: bool,
    alphabet: Alphabet,
    dotall: bool,
    literal_guards: bool,
    multiline: bool,
) -> Result<FixedPattern, String> {
    let (output, groups) = tokenize(
        pattern,
        verbose,
        alphabet,
        dotall,
        literal_guards,
        Grammar::Standard { multiline },
    )?;
    analyze(output, groups)
}

/// Normalize a text pattern requiring a lookaround without accepting backend-specific syntax.
pub fn lookahead_source(
    pattern: &str,
    verbose: bool,
    alphabet: Alphabet,
    dotall: bool,
) -> Result<(String, Vec<Option<String>>), String> {
    tokenize(
        pattern,
        verbose,
        alphabet,
        dotall,
        false,
        Grammar::Lookahead,
    )
}

/// Translate a pattern needing native backtracking, retaining Python capture slots.
/// Bytes patterns use this matcher for references, forward assertions and atomic operations.
pub fn backtracking_source(
    pattern: &str,
    verbose: bool,
    alphabet: Alphabet,
    dotall: bool,
) -> Result<(String, Vec<Option<String>>), String> {
    tokenize(
        pattern,
        verbose,
        alphabet,
        dotall,
        false,
        Grammar::Backtracking,
    )
}

/// Translate Python syntax for validation without requiring a particular matcher.
pub fn validation_source(
    pattern: &str,
    verbose: bool,
    alphabet: Alphabet,
    dotall: bool,
) -> Result<(String, Vec<Option<String>>), String> {
    tokenize(
        pattern,
        verbose,
        alphabet,
        dotall,
        false,
        Grammar::Validation,
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Grammar {
    Validation,
    Standard { multiline: bool },
    Lookahead,
    Backtracking,
}

enum Scope {
    Plain,
    Capture(usize),
    Conditional { has_else: bool },
    Lookahead,
    Lookbehind { first_local_group: usize },
}

// Widths count Python characters rather than UTF-8 bytes. Saturation also
// covers widths beyond Python's maximum representable lookbehind distance.
const VARIABLE_WIDTH: u64 = u64::MAX;

#[derive(Clone, Copy, Default)]
struct Width {
    min: u64,
    max: u64,
}

impl Width {
    const UNKNOWN: Self = Self {
        min: 0,
        max: VARIABLE_WIDTH,
    };

    fn fixed(n: u64) -> Self {
        Self { min: n, max: n }
    }

    fn exact(self) -> u64 {
        if self.min == self.max {
            self.min
        } else {
            VARIABLE_WIDTH
        }
    }
}

#[derive(Default)]
struct BranchWidth {
    prefix: Width,
    atom: Width,
    previous: Option<Width>,
}

impl BranchWidth {
    fn push(&mut self, atom: Width) {
        self.prefix.min = self.prefix.min.saturating_add(self.atom.min);
        self.prefix.max = self.prefix.max.saturating_add(self.atom.max);
        self.atom = atom;
    }

    fn width(&self) -> Width {
        let mut min = self.prefix.min.saturating_add(self.atom.min);
        let mut max = self.prefix.max.saturating_add(self.atom.max);
        if let Some(previous) = self.previous {
            min = min.min(previous.min);
            max = max.max(previous.max);
        }
        Width { min, max }
    }

    fn finish_branch(&mut self) {
        *self = Self {
            previous: Some(self.width()),
            ..Self::default()
        };
    }
}

struct GroupState {
    alphabet: Alphabet,
    backtrack: bool,
    dotall: bool,
    verbose: bool,
    atomic_assertion: bool,
    width: BranchWidth,
    scope: Scope,
}

/// Python can retain an earlier end slot while a repeated capture is open.
/// The backend tests only the start slot, so reject open conditions as well
/// as Python's invalid backreferences to an open group.
fn check_reference(id: usize, open: &[GroupState]) -> Result<(), String> {
    if id == 0
        || open.iter().any(|g| match g.scope {
            Scope::Capture(n) => n == id,
            Scope::Lookbehind { first_local_group } => id >= first_local_group,
            _ => false,
        })
    {
        return Err("invalid or open regex group reference".into());
    }
    Ok(())
}

fn reference_id(
    input: &mut Input<'_>,
    groups: &[Option<String>],
    conditional: bool,
) -> Result<usize, String> {
    let name: String = std::iter::from_fn(|| input.next_if(|c| *c != ')')).collect();
    if input.next() != Some(')') {
        return Err("unterminated group reference".into());
    }
    if conditional && !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
        return name
            .parse()
            .map_err(|_| "invalid regex group reference".into());
    }
    groups
        .iter()
        .position(|g| g.as_deref() == Some(&name))
        .ok_or_else(|| "unknown regex group name".into())
}

fn skip_comment(input: &mut impl Iterator<Item = char>) -> Result<(), String> {
    // The closing parenthesis must be unescaped, even inside verbose patterns.
    loop {
        match input.next() {
            Some(')') => return Ok(()),
            Some('\\') => {
                input.next().ok_or("bad escape (end of pattern)")?;
            }
            Some(_) => {}
            None => return Err("missing ), unterminated comment".into()),
        }
    }
}

fn tokenize(
    pattern: &str,
    mut verbose: bool,
    alphabet: Alphabet,
    mut dotall: bool,
    literal_guards: bool,
    grammar: Grammar,
) -> Result<(String, Vec<Option<String>>), String> {
    let lookahead = !matches!(grammar, Grammar::Standard { .. });
    let mut escapes = Escapes {
        alphabet,
        bracketed_word: false,
    };
    let mut modes = vec![(0, alphabet)];
    let mut scoped_mode = false;
    // Earlier captures can provide the width of a later lookbehind backreference.
    let needs_lookbehind_width = lookahead && pattern.contains("(?<");
    let mut has_lookaround = false;
    let mut has_lookbehind = false;
    let mut has_reference = false;
    let mut has_atomic = false;
    let mut nullable_repeat = false;
    let mut width = BranchWidth::default();
    let mut group_widths = Vec::new(); // Indexed by capture number minus one.
    let mut has_boundary = false;
    let mut highest_condition = 0;
    let mut literal_brace = false;
    let mut input = pattern.chars().peekable();
    let mut output = String::new();
    let mut groups = vec![None]; // Group zero is the whole match.
    let mut can_repeat = false;
    let mut may_backtrack = false;
    let mut outer_prefixes = Vec::new();
    while let Some(c) = next_token(&mut input, verbose)? {
        let atom_width = match c {
            '(' => {
                let extension = input.next_if_eq(&'?').is_some();
                if extension && input.next_if_eq(&'#').is_some() {
                    skip_comment(&mut input)?;
                    continue;
                }
                if literal_guards
                    && escapes.alphabet != Alphabet::UnicodeIgnoreCase
                    && extension
                    && input.next_if_eq(&'!').is_some()
                {
                    guarded_class(&mut input, &mut output, verbose, &mut escapes)?;
                    // A following quantifier would repeat the consumed class alone.
                    can_repeat = false;
                    width.push(Width::fixed(1));
                    continue;
                }
                // A standalone (?x) may follow only earlier global flags and ignored layout.
                if extension && input.clone().take(2).eq(['x', ')']) {
                    if !output.is_empty() {
                        return Err("global flags not at the start of the expression".into());
                    }
                    input.next();
                    input.next();
                    verbose = true;
                    continue;
                }
                if lookahead && extension && input.clone().take(2).eq(['P', '=']) {
                    input.next();
                    input.next();
                    let id = reference_id(&mut input, &groups, false)?;
                    check_reference(id, &outer_prefixes)?;
                    has_reference = true;
                    write!(output, r"\k<{id}>").unwrap();
                    can_repeat = true;
                    width.push(group_widths[id - 1]);
                    continue;
                }
                let behind = lookahead && extension && input.next_if_eq(&'<').is_some();
                let assertion = if lookahead && extension {
                    input.next_if(|c| matches!(c, '=' | '!'))
                } else {
                    None
                };
                if behind && assertion.is_none() {
                    return Err("expected Python lookbehind assertion".into());
                }
                let atomic_assertion = assertion == Some('=');
                let mut group = GroupState {
                    alphabet: escapes.alphabet,
                    backtrack: may_backtrack,
                    dotall,
                    verbose,
                    atomic_assertion,
                    scope: Scope::Plain,
                    width: std::mem::take(&mut width),
                };
                may_backtrack = false;
                output.push('(');
                if extension {
                    if let Some(assertion) = assertion {
                        has_lookaround = true;
                        output.push('?');
                        group.scope = if behind {
                            has_lookbehind = true;
                            output.push('<');
                            Scope::Lookbehind {
                                first_local_group: groups.len(),
                            }
                        } else {
                            Scope::Lookahead
                        };
                        output.push(assertion);
                        if atomic_assertion {
                            // CPython commits to the first successful positive assertion.
                            output.push_str("(?>");
                        }
                    } else if lookahead && input.next_if_eq(&'(').is_some() {
                        let id = reference_id(&mut input, &groups, true)?;
                        check_reference(id, &outer_prefixes)?;
                        highest_condition = highest_condition.max(id);
                        write!(output, "?({id})").unwrap();
                        group.scope = Scope::Conditional { has_else: false };
                    } else if input.next_if_eq(&':').is_some() {
                        output.push_str("?:");
                    } else if matches!(input.peek(), Some('a' | 'u' | 'i' | 's' | 'x' | '-')) {
                        let enable = input.next_if_eq(&'-').is_none();
                        let flag = input.next().ok_or("expected scoped flag")?;
                        if input.next() != Some(':') {
                            return Err("expected a single scoped flag".into());
                        }
                        let opener = match flag {
                            'x' => {
                                verbose = enable;
                                "?:"
                            }
                            's' => {
                                dotall = enable;
                                if enable { "?s:" } else { "?-s:" }
                            }
                            'a' | 'u' | 'i' => {
                                scoped_mode = true;
                                let parent = escapes.alphabet;
                                let next = parent.scoped(flag, enable)?;
                                if next != parent {
                                    escapes.alphabet = next;
                                    modes.push((output.len(), next));
                                }
                                // Only Unicode ignorecase uses the engine's fold.
                                // Even under ASCII, text dot and negated classes consume scalars.
                                match (parent, next) {
                                    (Alphabet::UnicodeIgnoreCase, n) if n != parent => "?-i:",
                                    (p, Alphabet::UnicodeIgnoreCase) if p != next => "?i:",
                                    _ => "?:",
                                }
                            }
                            _ => return Err("unsupported scoped flag".into()),
                        };
                        output.push_str(opener);
                    } else if input.next_if_eq(&'>').is_some() {
                        if grammar == Grammar::Backtracking {
                            has_atomic = true;
                            output.push_str("?>");
                        } else {
                            atomic_delimited(
                                &mut input,
                                &mut output,
                                dotall,
                                verbose,
                                &mut escapes,
                            )?;
                            may_backtrack = group.backtrack;
                            width = group.width;
                            width.push(Width::UNKNOWN);
                            can_repeat = true;
                            continue;
                        }
                    } else if input.next_if_eq(&'P').is_some() && input.next_if_eq(&'<').is_some() {
                        let mut name = String::new();
                        loop {
                            match input.next() {
                                Some('>') if !name.is_empty() => break,
                                Some(c)
                                    if lookahead && c != '>'
                                        || c == '_'
                                        || c.is_ascii_alphabetic()
                                        || !name.is_empty() && c.is_ascii_digit() =>
                                {
                                    name.push(c)
                                }
                                _ => return Err("expected an ASCII Python capture name".into()),
                            }
                        }
                        // The text lookahead backend uses the original Python names
                        // separately, including identifiers outside its own name syntax.
                        if !lookahead {
                            write!(output, "?P<{name}>").unwrap();
                        }
                        group.scope = Scope::Capture(groups.len());
                        groups.push(Some(name));
                        group_widths.push(Width::UNKNOWN);
                    } else {
                        return Err("unsupported regex group, assertion, or inline flag".into());
                    }
                } else {
                    group.scope = Scope::Capture(groups.len());
                    groups.push(None);
                    group_widths.push(Width::UNKNOWN);
                }
                outer_prefixes.push(group);
                can_repeat = false;
                continue;
            }
            '|' => {
                // regex-syntax factors identical prefixes. Moving a repetition
                // or nested choice before the branch can change the first match.
                // An empty character class is an unreachable branch that keeps
                // these alternatives separate without adding a capture.
                if let Some(group) = outer_prefixes.last_mut() {
                    group.backtrack = true;
                    if let Scope::Conditional { has_else } = &mut group.scope {
                        if *has_else {
                            return Err("conditional backref with more than two branches".into());
                        }
                        *has_else = true;
                        may_backtrack = false; // A conditional cannot take an extra unreachable branch.
                    }
                }
                output.push('|');
                if may_backtrack {
                    // A synthetic branch must preserve the backend's fixed-width
                    // analysis inside lookbehinds and the captures they reference.
                    let size = if needs_lookbehind_width {
                        width.width().exact()
                    } else {
                        1
                    };
                    if size == 0 {
                        output.push_str("(?!)");
                    } else {
                        output.push_str(r"[^\s\S]");
                        if (2..u64::from(u32::MAX)).contains(&size) {
                            write!(output, "{{{size}}}").unwrap();
                        }
                    }
                    output.push('|');
                }
                may_backtrack = false;
                can_repeat = false;
                width.finish_branch();
                continue;
            }
            ')' => {
                let mut body_width = width.width();
                if let Some(group) = outer_prefixes.pop() {
                    match group.scope {
                        Scope::Lookbehind { .. } if body_width.exact() > u64::from(u32::MAX) => {
                            return Err("look-behind requires fixed-width pattern".into());
                        }
                        Scope::Lookbehind { .. } | Scope::Lookahead => body_width = Width::fixed(0),
                        Scope::Capture(id) => group_widths[id - 1] = body_width,
                        Scope::Conditional { has_else: false } => body_width.min = 0,
                        _ => {}
                    }
                    width = group.width;
                    may_backtrack |= group.backtrack;
                    dotall = group.dotall;
                    verbose = group.verbose;
                    if escapes.alphabet != group.alphabet {
                        escapes.alphabet = group.alphabet;
                        modes.push((output.len(), group.alphabet));
                    }
                    if group.atomic_assertion {
                        output.push(')');
                    }
                }
                output.push(c);
                can_repeat = true;
                body_width
            }
            '.' | '^' => {
                output.push(c);
                can_repeat = c == '.';
                Width::fixed(u64::from(can_repeat))
            }
            '$' if grammar != (Grammar::Standard { multiline: false }) => {
                // Literals are escaped, so the runtime can lower this anchor using its line mode.
                output.push('$');
                can_repeat = false;
                Width::fixed(0)
            }
            '$' if input.peek().is_none()
                || verbose && next_token(&mut input.clone(), true)?.is_none() =>
            {
                // Remember a consumed final newline so the runtime can exclude it
                // from group zero. Dots are not valid in Python group names.
                output.push_str(FINAL_NEWLINE_ANCHOR);
                can_repeat = false;
                Width::fixed(0)
            }
            '$' => return Err("the $ anchor is supported only at the end of a pattern".into()),
            '[' => {
                class(&mut input, &mut output, &mut escapes)?;
                can_repeat = true;
                Width::fixed(1)
            }
            '\\' if lookahead && matches!(input.peek(), Some('b' | 'B')) => {
                // Compare the Python word status on either side without consuming a scalar.
                // The lookahead engine cannot switch its Unicode mode even for ASCII boundaries.
                has_boundary = true;
                let word = if escapes.alphabet.ascii() {
                    r"[A-Za-z0-9_]"
                } else if escapes.alphabet.ignores_case() {
                    r"(?-i:[\p{L}\p{N}_])"
                } else {
                    r"[\p{L}\p{N}_]"
                };
                let (after_word, after_nonword) = if input.next() == Some('b') {
                    ('!', '=')
                } else {
                    ('=', '!')
                };
                write!(
                    output,
                    "(?:(?<={word})(?{after_word}{word})|(?<!{word})(?{after_nonword}{word}))"
                )
                .unwrap();
                can_repeat = false;
                Width::fixed(0)
            }
            '\\' if lookahead && push_octal_escape(&mut input, &mut output, false)? => {
                can_repeat = true;
                Width::fixed(1)
            }
            '\\' if lookahead && matches!(input.peek(), Some('1'..='9')) => {
                let first = input.next().unwrap();
                let mut id = (first as u8 - b'0') as usize;
                if let Some(second) = input.next_if(char::is_ascii_digit) {
                    id = id * 10 + (second as u8 - b'0') as usize;
                }
                if id >= groups.len() {
                    return Err("invalid regex group reference".into());
                }
                check_reference(id, &outer_prefixes)?;
                has_reference = true;
                write!(output, r"\k<{id}>").unwrap();
                can_repeat = true;
                group_widths[id - 1]
            }
            '\\' => {
                let kind = escaped(&mut input, &mut output, false, &mut escapes)?;
                can_repeat = kind != Escape::Anchor;
                Width::fixed(u64::from(can_repeat))
            }
            '{' if !starts_counted_repeat(input.clone()) => {
                literal_brace = true;
                literal(&mut output, c);
                can_repeat = true;
                Width::fixed(1)
            }
            '*' | '+' | '?' | '{' => {
                may_backtrack = true;
                if !can_repeat {
                    return Err("regex quantifier requires a preceding atom".into());
                }
                output.push(c);
                let (lo, hi) = if c == '{' {
                    let lo = if input.peek() == Some(&',') {
                        output.push('0');
                        0
                    } else {
                        count(&mut input, &mut output)?
                            .ok_or("expected a compact regex repetition count")?
                    };
                    let hi = if input.next_if_eq(&',').is_some() {
                        output.push(',');
                        count(&mut input, &mut output)?.map_or(VARIABLE_WIDTH, u64::from)
                    } else {
                        u64::from(lo)
                    };
                    if input.next_if_eq(&'}').is_none() {
                        return Err("expected a compact regex repetition count".into());
                    }
                    if hi < u64::from(lo) {
                        return Err("min repeat greater than max repeat".into());
                    }
                    output.push('}');
                    (u64::from(lo), hi)
                } else {
                    (
                        u64::from(c == '+'),
                        if c == '?' { 1 } else { VARIABLE_WIDTH },
                    )
                };
                // Another optional iteration at the same offset can change the last capture.
                nullable_repeat |= width.atom.min == 0 && hi.saturating_sub(lo) > 1;
                width.atom.min = width.atom.min.saturating_mul(lo);
                width.atom.max = width.atom.max.saturating_mul(hi);
                // Modifiers must be adjacent even in verbose mode.
                if input.next_if_eq(&'?').is_some() {
                    output.push('?');
                } else if grammar == Grammar::Backtracking && input.next_if_eq(&'+').is_some() {
                    has_atomic = true;
                    output.push('+');
                }
                can_repeat = false;
                continue;
            }
            c => {
                literal(&mut output, c);
                can_repeat = true;
                Width::fixed(1)
            }
        };
        width.push(atom_width);
    }
    if (literal_brace || lookahead || scoped_mode) && escapes.bracketed_word {
        return Err(
            "case-insensitive Unicode word sets inside bracket expressions are unsupported".into(),
        );
    }
    if highest_condition >= groups.len() {
        return Err("invalid conditional group reference".into());
    }
    let casefold = modes.iter().any(|(_, mode)| mode.ignores_case());
    let text_assertions =
        alphabet != Alphabet::Bytes && (has_lookaround || scoped_mode || has_boundary);
    // Bytes lookbehind has its own bounded matcher. Keep nullable repeats out
    // of the forward-assertion fallback so a stalled iteration cannot change a capture.
    let byte_lookahead =
        alphabet == Alphabet::Bytes && has_lookaround && !has_lookbehind && !nullable_repeat;
    match grammar {
        Grammar::Lookahead if !has_lookaround => {
            return Err("expected a Python lookahead assertion".into());
        }
        Grammar::Backtracking
            if !has_reference
                && !text_assertions
                && !byte_lookahead
                && !(has_atomic && (alphabet != Alphabet::Bytes || !has_lookbehind)) =>
        {
            return Err("pattern does not require backtracking".into());
        }
        // Keep previously supported case-sensitive text assertions on their existing matcher.
        // For newly admitted references, a stalled repeat could leave the wrong capture.
        Grammar::Backtracking
            if nullable_repeat
                && (has_atomic || has_reference && (!text_assertions || casefold)) =>
        {
            return Err("a repeated empty match could change the referenced capture".into());
        }
        _ => {}
    }
    if casefold {
        output = if grammar == Grammar::Backtracking && has_reference {
            casefold::backrefs(&output, &modes)?
        } else {
            casefold::scoped(&output, &modes, lookahead)?
        };
    }
    Ok((output, groups))
}

fn analyze(output: String, groups: Vec<Option<String>>) -> Result<FixedPattern, String> {
    let output = preserve_zero_repeat_groups(output)?;
    let hir = regex_syntax::Parser::new()
        .parse(&output)
        .map_err(|e| e.to_string())?;
    let mut required = vec![false; groups.len()];
    required[0] = true;
    let mut nodes = vec![(&hir, true)];
    while let Some((node, always)) = nodes.pop() {
        if let HirKind::Repetition(repeat) = node.kind()
            && repeat.max != Some(1)
            && (repeat.sub.properties().minimum_len() == Some(0)
                || repeat.sub.properties().explicit_captures_len() != 0
                    && !matches!(repeat.sub.kind(), HirKind::Capture(c) if c.sub.properties().explicit_captures_len() == 0))
        {
            return Err("repeating a nullable expression or capture is not supported".into());
        }
        let always = match node.kind() {
            HirKind::Capture(capture) => {
                // The hidden final-newline capture is beyond Python's group count.
                if let Some(required) = required.get_mut(capture.index as usize) {
                    *required = always;
                }
                always
            }
            HirKind::Repetition(repeat) => always && repeat.min > 0,
            HirKind::Alternation(_) => false,
            _ => always,
        };
        nodes.extend(node.kind().subs().iter().map(|sub| (sub, always)));
    }
    let regex = regex::Regex::new(&output).map_err(|e| e.to_string())?;
    if !regex
        .capture_names()
        .filter(|name| *name != Some("imre.eol"))
        .eq(groups.iter().map(Option::as_deref))
    {
        return Err("regex optimization would change the Python capture groups".into());
    }
    Ok(FixedPattern {
        source: output,
        may_match_empty: hir.properties().minimum_len() == Some(0),
        groups: groups
            .into_iter()
            .zip(required)
            .map(|(name, required)| Capture { name, required })
            .collect(),
    })
}

mod replacement;
pub use replacement::{ReplacementError, ReplacementPart, parse_replacement, replacement};

type Input<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Does the text after an unescaped opening brace form a Python repetition count?
/// Only a complete ASCII bound (including an omitted bound in `{,}`) is a quantifier.
pub fn starts_counted_repeat(mut input: Input<'_>) -> bool {
    if !matches!(input.peek(), Some('0'..='9' | ',')) {
        return false;
    }
    let mut suffix = input.skip_while(char::is_ascii_digit);
    match suffix.next() {
        Some('}') => true,
        Some(',') => suffix.find(|c| !c.is_ascii_digit()) == Some('}'),
        _ => false,
    }
}

/// Find a quantified anchor or repeated quantifier in a pattern accepted by the raw backend.
/// Offsets are byte indices into the Python spelling, before escape or alphabet translation.
pub fn repeat_diagnostic(source: &str, verbose: bool) -> Option<(&'static str, usize)> {
    use regex_syntax::ast::{Ast, parse::ParserBuilder};
    use std::borrow::Cow;

    let layout = verbose
        || source.match_indices("(?").any(|(i, _)| {
            source[i + 2..]
                .bytes()
                .take_while(|b| b"aiLmsux-".contains(b))
                .any(|b| b == b'x')
        });
    if !layout
        && !source
            .as_bytes()
            .windows(2)
            .any(|p| b"*+?}^$AbBZz".contains(&p[0]) && b"*+?{".contains(&p[1]))
    {
        return None;
    }
    // Rust and Python can disagree about which characters are tokens in these verbose
    // patterns. Leave those to the Python tokenizer rather than reject a literal or comment.
    if layout
        && (source.contains('[')
            || source.contains("\\\n")
            || source.chars().any(|c| !c.is_ascii() && c.is_whitespace()))
    {
        return None;
    }
    let spelling = if source.contains(r"\Z") {
        Cow::Owned(source.replace(r"\Z", r"\z"))
    } else {
        Cow::Borrowed(source)
    };
    let ast = ParserBuilder::new()
        .ignore_whitespace(verbose)
        .empty_min_range(true)
        .build()
        .parse(&spelling)
        .ok()?;
    let mut first = None;
    let mut pending = vec![&ast];
    while let Some(node) = pending.pop() {
        match node {
            Ast::Repetition(outer) => {
                // Parentheses make a new operand. Only adjacent repetitions share this span.
                let mut inner = outer.as_ref();
                while let Ast::Repetition(child) = inner.ast.as_ref() {
                    inner = child;
                }
                pending.push(&inner.ast);
                let start = inner.op.span.start.offset;
                let end = outer.op.span.end.offset;
                let mut chars = source[start..end].chars().peekable();
                if chars.next() == Some('{') {
                    if !starts_counted_repeat(chars.clone()) {
                        continue;
                    }
                    chars.by_ref().find(|&c| c == '}');
                }
                // The backend also calls escaped angles and \b{start} assertions.
                // Those spellings contain repeatable literals in Python.
                let anchor = matches!(inner.ast.as_ref(), Ast::Assertion(a) if matches!(
                    &source[a.span.start.offset..a.span.end.offset],
                    "^" | "$" | r"\A" | r"\Z" | r"\z" | r"\b" | r"\B"));
                let error = if anchor {
                    ("nothing to repeat", start)
                } else {
                    // Python permits exactly one adjacent lazy or possessive modifier.
                    chars.next_if(|c| matches!(c, '?' | '+'));
                    match next_token(&mut chars, layout) {
                        Ok(Some('*' | '+' | '?')) => {}
                        Ok(Some('{')) if starts_counted_repeat(chars.clone()) => {}
                        _ => continue,
                    }
                    (
                        "multiple repeat",
                        end - chars.map(char::len_utf8).sum::<usize>() - 1,
                    )
                };
                if first.is_none_or(|(_, pos)| error.1 < pos) {
                    first = Some(error);
                }
            }
            Ast::Group(g) => pending.push(&g.ast),
            Ast::Concat(c) => pending.extend(c.asts.iter().rev()),
            Ast::Alternation(a) => pending.extend(a.asts.iter().rev()),
            _ => {}
        }
    }
    first
}

fn count(input: &mut Input<'_>, output: &mut String) -> Result<Option<u32>, String> {
    let digits: String = std::iter::from_fn(|| input.next_if(char::is_ascii_digit)).collect();
    if digits.is_empty() {
        return Ok(None);
    }
    // Python reserves UINT32_MAX to mean unbounded repetition.
    let value = digits
        .parse::<u32>()
        .ok()
        .filter(|&n| n != u32::MAX)
        .ok_or("regex repetition count is too large")?;
    output.push_str(&digits);
    Ok(Some(value))
}

/// Skip ignored VERBOSE layout outside character classes.
fn next_token(input: &mut Input<'_>, verbose: bool) -> Result<Option<char>, String> {
    while let Some(c) = input.next() {
        if !verbose || !(verbose_space(c) || c == '#') {
            return Ok(Some(c));
        }
        if c == '#' {
            while let Some(ch) = input.next() {
                match ch {
                    '\\' => {
                        input.next().ok_or("incomplete regex escape in comment")?;
                    }
                    '\n' => break,
                    _ => {}
                }
            }
        }
    }
    Ok(None)
}

/// A negative choice of single literals directly before one class excludes those
/// characters from the class. Captures and multi-character alternatives need a real assertion.
fn guarded_class(
    input: &mut Input<'_>,
    output: &mut String,
    verbose: bool,
    escapes: &mut Escapes,
) -> Result<(), String> {
    let unsupported =
        "negative regex guard requires single literal alternatives followed by a class";
    let mut excluded = String::new();
    loop {
        match next_token(input, verbose)? {
            Some('\\') if escaped(input, &mut excluded, false, escapes)? == Escape::Scalar => {}
            Some(c) if !r"\.^$*+?{}[]()|".contains(c) => literal(&mut excluded, c),
            _ => return Err(unsupported.into()),
        }
        match next_token(input, verbose)? {
            Some('|') => {}
            Some(')') => break,
            _ => return Err(unsupported.into()),
        }
    }
    output.push('[');
    match next_token(input, verbose)? {
        Some('[') => class(input, output, escapes)?,
        Some('\\') if escaped(input, output, false, escapes)? == Escape::Class => {}
        _ => return Err(unsupported.into()),
    }
    write!(output, "--[{excluded}]]").unwrap();
    Ok(())
}

/// A lazy scan inside an atomic group commits to its first matching delimiter.
/// Removing those characters from the repetition prevents a later suffix from
/// making the engine reconsider that choice, including across newlines under DOTALL.
fn atomic_delimited(
    input: &mut Input<'_>,
    output: &mut String,
    dotall: bool,
    verbose: bool,
    escapes: &mut Escapes,
) -> Result<(), String> {
    if !['.', '*', '?'].into_iter().all(|c| input.next() == Some(c)) {
        return Err("unsupported atomic regex group".into());
    }
    let mut delimiter = String::new();
    match input.next() {
        Some('[') => class(input, &mut delimiter, escapes)?,
        Some('\\') => {
            // The delimiter also appears inside a negated class in the lowered scan.
            let word_delimiter = matches!(input.peek(), Some('w' | 'W'));
            if escaped(input, &mut delimiter, word_delimiter, escapes)? == Escape::Anchor {
                return Err("an atomic scan delimiter must consume a character".into());
            }
        }
        Some(c) if !".^$*+?{}()|".contains(c) && !(verbose && (verbose_space(c) || c == '#')) => {
            literal(&mut delimiter, c);
        }
        _ => return Err("expected a single-character delimiter in an atomic regex group".into()),
    }
    if input.next() != Some(')') {
        return Err("unsupported atomic regex group".into());
    }
    let newline = if dotall { "" } else { r"\n" };
    write!(output, "?:[^{delimiter}{newline}]*{delimiter})").unwrap();
    Ok(())
}

fn verbose_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// Emitting punctuation as a scalar prevents Rust-only syntax from appearing
/// when Python literal characters become adjacent after comment removal.
fn literal(output: &mut String, c: char) {
    if c.is_alphanumeric() || c == '_' {
        output.push(c);
    } else {
        write!(output, "\\x{{{:x}}}", c as u32).unwrap();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Escape {
    Scalar,
    Class,
    Anchor,
}

/// Consume a Python pattern octal and emit one scalar (or one octet in a bytes pattern).
/// Outside a class, a nonzero escape with fewer than three octal digits is a group reference.
fn push_octal_escape(
    input: &mut Input<'_>,
    output: &mut String,
    in_class: bool,
) -> Result<bool, String> {
    let Some(&first @ '0'..='7') = input.peek() else {
        return Ok(false);
    };
    let mut digits = input.clone();
    let mut value = 0u16;
    let mut count = 0;
    while count < 3
        && let Some(c) = digits.next_if(|c| matches!(c, '0'..='7'))
    {
        value = value * 8 + (c as u16 - u16::from(b'0'));
        count += 1;
    }
    if !in_class && first != '0' && count < 3 {
        return Ok(false);
    }
    if value > 0xff {
        return Err(format!(
            r"octal escape value \{value:o} outside of range 0-0o377"
        ));
    }
    *input = digits;
    write!(output, r"\x{value:02x}").unwrap();
    Ok(true)
}

fn escaped(
    input: &mut Input<'_>,
    output: &mut String,
    in_class: bool,
    escapes: &mut Escapes,
) -> Result<Escape, String> {
    if push_octal_escape(input, output, in_class)? {
        return Ok(Escape::Scalar);
    }
    let c = input.next().ok_or("incomplete regex escape")?;
    let ascii = escapes.alphabet.ascii();
    if ascii && let Some(class) = ascii_class(c) {
        output.push_str(class);
        return Ok(Escape::Class);
    }
    let value = match c {
        'b' | 'B' if ascii && !in_class => {
            output.push_str(if c == 'b' { r"(?-u:\b)" } else { r"(?-u:\B)" });
            return Ok(Escape::Anchor);
        }
        'w' | 'W' => {
            // Python uses Unicode letters, numbers and the literal underscore.
            // Rust's shorthand additionally includes marks, connector characters,
            // and join controls, all of which Python treats as non-word.
            let protect_word = escapes.alphabet == Alphabet::UnicodeIgnoreCase && !in_class;
            if protect_word {
                output.push_str("(?-i:");
            }
            escapes.bracketed_word |= in_class && escapes.alphabet.ignores_case();
            output.push_str(if c == 'w' {
                r"[\p{L}\p{N}_]"
            } else {
                r"[^\p{L}\p{N}_]"
            });
            if protect_word {
                output.push(')');
            }
            return Ok(Escape::Class);
        }
        'd' | 'D' => {
            output.push_str(if c == 'd' { r"\p{Nd}" } else { r"\P{Nd}" });
            return Ok(Escape::Class);
        }
        's' | 'S' => {
            output.push('[');
            if c == 'S' {
                output.push('^');
            }
            output.push_str(SPACE);
            output.push(']');
            return Ok(Escape::Class);
        }
        'A' | 'Z' | 'z' if !in_class => {
            output.push_str(if c == 'A' { r"\A" } else { r"\z" });
            return Ok(Escape::Anchor);
        }
        'a' => '\x07',
        'b' if in_class => '\x08',
        't' => '\t',
        'n' => '\n',
        'v' => '\x0b',
        'f' => '\x0c',
        'r' => '\r',
        'u' | 'U' if escapes.alphabet == Alphabet::Bytes => {
            return Err(format!("bad escape \\{c} in bytes pattern"));
        }
        'x' | 'u' | 'U' => {
            let digits = match c {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            let mut result = 0;
            for _ in 0..digits {
                let next = input
                    .next()
                    .and_then(|c| c.to_digit(16))
                    .ok_or("invalid hexadecimal regex escape")?;
                result = result * 16 + next;
            }
            if c == 'x' {
                // The same escape denotes a scalar in text and a single octet in bytes.
                write!(output, "\\x{result:02x}").unwrap();
                return Ok(Escape::Scalar);
            }
            char::from_u32(result).ok_or("regex escape is not a Unicode scalar")?
        }
        c if c.is_ascii_punctuation() || verbose_space(c) => c,
        _ => return Err(format!("unsupported Python regex escape \\{c}")),
    };
    literal(output, value);
    Ok(Escape::Scalar)
}

fn class(input: &mut Input<'_>, output: &mut String, escapes: &mut Escapes) -> Result<(), String> {
    output.push('[');
    // An immediate second '[' is CPython's possible nested set (FutureWarning).
    if input.peek() == Some(&'[') {
        return Err("nested or ambiguous regex character class is not supported".into());
    }
    if input.next_if_eq(&'^').is_some() {
        output.push('^');
    }
    let mut first = true;
    // Only a standalone atom can begin a range. Once both endpoints are
    // consumed, a following hyphen starts a new atom and is literal.
    let mut left_atom: Option<bool> = None;
    let mut range_pending = false;
    while let Some(c) = input.next() {
        if c == ']' && !first {
            output.push(']');
            return Ok(());
        }
        if matches!(c, '&' | '~' | '|' | '-') && input.peek() == Some(&c) {
            return Err("nested or ambiguous regex character class is not supported".into());
        }
        if c == '-'
            && let Some(scalar) = left_atom
            && input.peek() != Some(&']')
        {
            if !scalar {
                return Err("regex range requires scalar endpoints".into());
            }
            output.push('-');
            left_atom = None;
            range_pending = true;
            continue;
        }
        let scalar = if c == '\\' {
            escaped(input, output, true, escapes)? == Escape::Scalar
        } else {
            literal(output, c);
            true
        };
        if range_pending && !scalar {
            return Err("regex range requires scalar endpoints".into());
        }
        left_atom = (!range_pending).then_some(scalar);
        range_pending = false;
        first = false;
    }
    Err("unclosed regex character class".into())
}

#[cfg(test)]
mod tests {
    use super::translate;
    use regex::Regex;

    #[test]
    fn scoped_text_modes_restore_classes_and_case_equivalents() {
        use super::{Alphabet::*, translate_pattern};

        for (mode, pattern, yes, no) in [
            (
                UnicodeIgnoreCase,
                r"é(?a:(?P<letter>[a-z])(?u:\w))(?-i:I)[a-z]",
                "éAéIı",
                "éKéIı",
            ),
            (AsciiIgnoreCase, r"(?u:[a-z])(?a:[a-z])[^a-z]", "KKı", "KKı"),
            (Unicode, r"(?a:(?i:[^\x69])(?i:i))(?i:i)i", "İIıi", "iIıi"),
            (UnicodeIgnoreCase, r"(?a:(?u:ſ)ſ).", "Sſé", "SSé"),
            (Unicode, r"(?a:\w(?u:\w)\W).", "aéé🌍", "ééé🌍"),
            (UnicodeIgnoreCase, r"(?-i:I)(?a:\x6b)\u0130", "IKı", "ıKı"),
        ] {
            let translated = translate_pattern(pattern, false, mode, false, true).unwrap();
            let regex = regex::RegexBuilder::new(&format!(r"\A(?:{})\z", translated.source))
                .case_insensitive(mode == UnicodeIgnoreCase)
                .build()
                .unwrap();
            assert!(regex.is_match(yes), "{pattern} on {yes}");
            assert!(!regex.is_match(no), "{pattern} on {no}");
        }
        for pattern in [r"(?-a:a)", r"(?au:a)"] {
            assert!(
                translate_pattern(pattern, false, Unicode, false, false).is_err(),
                "{pattern}"
            );
        }
        assert!(super::lookahead_source(r"(?i:(?=\w)(a)\1)", false, Unicode, false).is_err());
        assert!(translate_pattern(r"(?u:.)", false, Bytes, false, false).is_err());
        let named = "(?P<é>.)(?-i:I)";
        assert!(super::lookahead_source(named, false, UnicodeIgnoreCase, false).is_err());
        assert!(super::lookahead_source(r"(?u:\b)", false, Unicode, false).is_err());
        let (source, groups) =
            super::backtracking_source(named, false, UnicodeIgnoreCase, false).unwrap();
        assert_eq!(groups, [None, Some("é".to_owned())]);
        let compiled = regex::RegexBuilder::new(&source)
            .case_insensitive(true)
            .build()
            .unwrap();
        assert!(compiled.is_match("aI"));
        assert!(!compiled.is_match("aİ"));
    }

    #[test]
    fn subtracts_single_literal_guards_from_consuming_classes() {
        let opts = r"(?P<option>(?:(?!=|:)\S)*(?:\s+(?:(?!=|:)\S)+)*)
                     \s*(?P<vi>=|:)\s*(?P<value>.*)$";
        let regex = Regex::new(&translate(opts, true).unwrap().source).unwrap();
        let caps = regex.captures("two words : =value").unwrap();
        assert_eq!(
            (&caps["option"], &caps["vi"], &caps["value"]),
            ("two words", ":", "=value")
        );
        let spaced = r"(?:(?! \# |  # a second excluded literal
                         \| ) [^!])+";
        let regex = Regex::new(&translate(spaced, true).unwrap().source).unwrap();
        let runs: Vec<_> = regex.find_iter("a#b|c!d").map(|m| m.as_str()).collect();
        assert_eq!(runs, ["a", "b", "c", "d"]);
        for (ascii, expected) in [(false, "a"), (true, "a\u{1c}b\u{a0}c")] {
            let pattern =
                super::translate_with_flags(r"(?:(?!\x3a)\S)+", false, ascii, false).unwrap();
            let regex = Regex::new(&pattern.source).unwrap();
            assert_eq!(regex.find("a\u{1c}b\u{a0}c:d").unwrap().as_str(), expected);
        }
        for unsupported in [
            r"(?!ab)\S",
            r"(?!a|)\S",
            r"(?![ab])\S",
            r"(?!(a))\S",
            r"(?!\d)\S",
            r"(?!a)b",
            r"(?!a)\S+",
            r"(?!a)\S {1,2}",
        ] {
            assert!(translate(unsupported, true).is_err(), "{unsupported}");
        }
    }

    #[test]
    fn preserves_verbose_class_contents_and_python_whitespace() {
        let pattern = r"\# \s* E # comment
             (?P<tag>[ #\]]+)";
        let translated = translate(pattern, true).unwrap();
        assert_eq!(
            translated
                .groups
                .iter()
                .map(|g| g.name.as_deref())
                .collect::<Vec<_>>(),
            [None, Some("tag")]
        );
        let regex = Regex::new(&translated.source).unwrap();
        let caps = regex.captures("🌍#\u{1c}\u{85}E #]z").unwrap();
        assert_eq!(&caps["tag"], " #]");
        let regex = Regex::new(&translate(r"[\s][^\s]\S", false).unwrap().source).unwrap();
        assert!(regex.is_match("\u{1f}xé"));
        assert!(!regex.is_match("\u{200b}xé"));
        // Only the six ASCII VERBOSE whitespace characters are ignored.
        assert!(
            Regex::new(&translate("a\u{a0}b", true).unwrap().source)
                .unwrap()
                .is_match("a\u{a0}b")
        );
    }

    #[test]
    fn escaped_newline_inside_verbose_comment_is_still_comment_text() {
        for (slashes, expected) in [(1, "a"), (2, "ab"), (3, "a")] {
            let pattern = format!("a#comment{}{}b", "\\".repeat(slashes), '\n');
            assert_eq!(
                Regex::new(&translate(&pattern, true).unwrap().source)
                    .unwrap()
                    .find("ab")
                    .unwrap()
                    .as_str(),
                expected
            );
        }
        assert!(translate("a#comment\\", true).is_err());
        assert!(translate("(?:){4294967295}", false).is_err());
    }

    #[test]
    fn hyphen_after_a_complete_range_is_literal() {
        for (pattern, expected) in [(r"[a-b-c]", "abc-"), (r"[^a-b-c]", "dx")] {
            let regex = Regex::new(&translate(pattern, false).unwrap().source).unwrap();
            let matched: String = regex.find_iter("abc-dx").map(|m| m.as_str()).collect();
            assert_eq!(matched, expected);
        }
    }

    #[test]
    fn alternatives_keep_priority_over_a_shared_repetition_or_choice() {
        for (pattern, text, expected) in [
            (".*?b|.*?a", "ab", "ab"),
            (".*a|.*b", "ab", "a"),
            ("(?:a|ab)c|(?:a|ab)b", "abc", "abc"),
        ] {
            let compiled = Regex::new(&translate(pattern, false).unwrap().source).unwrap();
            assert_eq!(compiled.find(text).unwrap().as_str(), expected);
        }
    }

    #[test]
    fn atomic_lazy_scans_commit_to_the_first_delimiter() {
        for (pattern, matches, misses) in [
            (r"\A(?>.*?a)b", "zab", "aab"),
            (r"\A(?s:(?>.*?x)y)", "\nxy", "xxy"),
            (r"\A(?s:.)(?>.*?x)y", "\n.xy", "\n\nxy"),
            (r"\A(?>.*?[ab])x", "-ax", "abx"),
            (r"\A(?>.*?\+)x", "-+x", "++x"),
        ] {
            let regex = Regex::new(&translate(pattern, false).unwrap().source).unwrap();
            assert!(regex.is_match(matches));
            assert!(!regex.is_match(misses));
        }
        assert!(translate("(?>.*?a|b)c", false).is_err());
    }

    #[test]
    fn incomplete_counts_leave_the_brace_and_following_pattern_intact() {
        for (pattern, text) in [
            (r"^{\w+}$", "{width}"),
            (r"a{1, 3}", "a{1,3}"),
            (r"a{4294967295x", "a{4294967295x"),
            (r"a{１}", "a{１}"),
            (r"a{{2}", "a{{"),
            (r"{}?", "{"),
        ] {
            let regex = Regex::new(&translate(pattern, true).unwrap().source).unwrap();
            assert_eq!(regex.find(text).unwrap().as_str(), text, "{pattern}");
        }
        for pattern in ["{2}", "a{2,1}", "a*{2}", "a{4294967295}"] {
            assert!(translate(pattern, false).is_err(), "{pattern}");
        }
    }

    #[test]
    fn rejects_semantics_the_engine_does_not_share() {
        for source in [
            r"a$b",
            r"\1",
            r"\b",
            r"(?=a)",
            r"(?P<a>x)(?P=a)",
            r"(?au:a)",
            r"a*+",
            r"a* ?",
            r"((a)?b)+",
            r"(x?)*",
            r"[a&&b]",
            r"[[a]]",
            r"[\s-x]",
            r"[x-\s]",
            r"\uD800",
            r"(?P<1bad>x)",
        ] {
            assert!(translate(source, true).is_err(), "{source}");
        }
        for source in [
            r"\A(ab|c)(?P<maybe>z)?\Z",
            r"[a-z]{2,4}?",
            r"[\-\]]+",
            r"(?P<empty>x*)?",
            r"\w",
            r"[^\W]",
            r"(a)+",
        ] {
            assert!(translate(source, false).is_ok(), "{source}");
        }
    }
}
