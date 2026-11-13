use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::fmt::Write;
use std::rc::Rc;

use num_traits::ToPrimitive;
use regex_automata::{Anchored, Input, PatternID, Span, meta::Regex, util::captures::Captures};

use super::{Context, Error, Id, NONE, Result, Value, argument};
use crate::dynamic::builtins::bind;

mod assertions;
mod escapes;
mod flags;
mod group_names;
#[cfg(target_os = "linux")]
mod locale;
mod lookahead;
mod progress;
mod scanner;
mod split;
mod sub;
mod surrogate;

pub(super) use scanner::scanner_next;

use assertions::{Guard, Kind};
use progress::Matcher;
pub(super) use split::pattern_split;
use split::split;
pub(super) use sub::{match_expand, pattern_sub};

use imre_regex::flags::{ASCII, DEBUG, DOTALL, IGNORECASE, LOCALE, MULTILINE, UNICODE, VERBOSE};

const KNOWN_FLAGS: i64 =
    IGNORECASE | LOCALE | MULTILINE | DOTALL | UNICODE | VERBOSE | DEBUG | ASCII;

// Declaration order is also the order used by the module-qualified flag repr.
const FLAGS: &[(&str, &str, i64)] = &[
    ("NOFLAG", "", 0),
    ("ASCII", "A", ASCII),
    ("IGNORECASE", "I", IGNORECASE),
    ("LOCALE", "L", LOCALE),
    ("UNICODE", "U", UNICODE),
    ("MULTILINE", "M", MULTILINE),
    ("DOTALL", "S", DOTALL),
    ("VERBOSE", "X", VERBOSE),
    ("DEBUG", "", DEBUG),
];

pub(super) fn is_flag_attribute(name: &str) -> bool {
    name == "RegexFlag"
        || FLAGS
            .iter()
            .any(|&(long, short, _)| name == long || !short.is_empty() && name == short)
}

pub(super) fn flag_attribute(ctx: &mut Context, name: &str) -> Result<Id> {
    let class = if let Some(&class) = ctx.classes.get("re.RegexFlag") {
        class
    } else {
        let scope = ctx.scope("re", None);
        let module = ctx.text("re");
        ctx.bind(scope, "__module__", module);
        for &(long, short, bits) in FLAGS {
            let value = ctx.int(bits);
            ctx.bind(scope, long, value);
            if !short.is_empty() {
                ctx.bind(scope, short, value);
            }
        }
        let module = ctx.module("enum")?;
        let repr = ctx.lookup_attr(module, "global_flag_repr")?;
        ctx.bind(scope, "__repr__", repr);
        let hex = ctx.builtin("hex");
        ctx.bind(scope, "_numeric_repr_", hex);
        // CPython's RegexFlag explicitly uses object.__str__ to display its global name.
        let string = ctx.builtin("object.__str__");
        ctx.bind(scope, "__str__", string);
        let base = ctx.builtin("enum.IntFlag");
        let class = ctx.allocate_class("RegexFlag".into(), scope, vec![base])?;
        ctx.populate_enum(class, Some(base))?;
        ctx.classes.insert("re.RegexFlag".into(), class);
        class
    };
    if name == "RegexFlag" {
        Ok(class)
    } else {
        Ok(ctx.class_attr(class, name).expect("declared regex flag"))
    }
}

pub(in crate::dynamic) struct Pattern {
    pub(in crate::dynamic) source: Id,
    pub(in crate::dynamic) flags: i64,
    bytes: bool,
    engine_source: String,
    surrogate_literals: Option<Box<Pattern>>,
    regex: Matcher,
    assertions: Vec<(String, Guard)>,
    full: OnceCell<std::result::Result<Matcher, String>>,
    captures: OnceCell<Box<RefCell<Captures>>>,
    full_captures: OnceCell<Box<RefCell<Captures>>>,
    close_order: OnceCell<Option<Vec<usize>>>,
    split_retry: OnceCell<std::result::Result<regex_automata::nfa::thompson::NFA, String>>,
}

impl Matcher {
    fn capture_buffer<'a>(
        &self,
        storage: &'a OnceCell<Box<RefCell<Captures>>>,
    ) -> &'a RefCell<Captures> {
        storage.get_or_init(|| Box::new(RefCell::new(self.create_captures())))
    }
}

impl Pattern {
    /// Synthetic Scanner patterns do not have a Python pattern object to compare.
    pub(in crate::dynamic) fn same_program(&self, other: &Self) -> bool {
        if self.source == NONE
            || other.source == NONE
            || self.flags != other.flags
            || self.bytes != other.bytes
            || self.assertions.len() != other.assertions.len()
            || !self
                .assertions
                .iter()
                .zip(&other.assertions)
                .all(|((a, x), (b, y))| {
                    a == b && x.kind == y.kind && x.width == y.width && x.program == y.program
                })
        {
            return false;
        }
        // Scalar-only subjects cannot match any surrogate literal. The alternate
        // program distinguishes the actual code points on surrogate-bearing subjects.
        match (&self.surrogate_literals, &other.surrogate_literals) {
            (None, None) => {}
            (Some(a), Some(b)) if a.same_program(b) => {}
            _ => return false,
        }
        #[cfg(target_os = "linux")]
        if let (Matcher::Locale(a), Matcher::Locale(b)) = (&self.regex, &other.regex) {
            return a.same_program(b);
        }
        if self.engine_source == other.engine_source {
            return true;
        }
        // str subclasses may compare equal despite different spellings. Compare their
        // compiled states, independent of spelling and capture names, before calling __eq__.
        !matches!(self.regex, Matcher::Lookahead(_))
            && !matches!(other.regex, Matcher::Lookahead(_))
            && matches!((split::retry_nfa(self), split::retry_nfa(other)), (Ok(a), Ok(b)) if a.states() == b.states())
    }

    pub(in crate::dynamic) fn program_hash(&self) -> i64 {
        // Equal state programs can have different spellings (for example noncapturing groups).
        // The shared group layout is also invariant under capture renaming.
        self.flags ^ i64::from(self.bytes) ^ (self.group_count() as i64).rotate_left(7)
    }

    // GroupInfo contains the synthetic final-newline capture as well as user names.
    fn invalid_group_name(&self, spelling: &[u8]) -> Option<&str> {
        let mut names = self
            .regex
            .group_info()
            .pattern_names(PatternID::ZERO)
            .flatten();
        names.find(|name| {
            (*name != "imre.eol"
                || spelling
                    .windows(b"(?P<imre.eol>".len())
                    .any(|w| w == b"(?P<imre.eol>"))
                && !group_names::valid(name, self.bytes)
        })
    }

    pub(in crate::dynamic) fn repr(&self, ctx: &Context) -> String {
        let mut source = ctx.repr(self.source);
        if let Some((end, _)) = source.char_indices().nth(200) {
            source.truncate(end);
        }
        let flags = if self.bytes || self.source == NONE {
            self.flags
        } else {
            self.flags & !UNICODE
        };
        if flags == 0 {
            format!("re.compile({source})")
        } else {
            let mut names: Vec<_> = FLAGS
                .iter()
                .filter(|(_, _, flag)| flags & flag != 0)
                .collect();
            names.sort_unstable_by_key(|(_, _, flag)| flag);
            let mut labels = names
                .into_iter()
                .map(|(name, _, _)| format!("re.{name}"))
                .collect::<Vec<_>>();
            let remaining = (flags & !KNOWN_FLAGS) as u32;
            if remaining != 0 {
                labels.push(format!("0x{remaining:x}"));
            }
            format!("re.compile({source}, {})", labels.join("|"))
        }
    }

    /// Describe the matcher and translated program that this Pattern will execute.
    fn debug_program(&self, output: &mut String) {
        let source: &dyn std::fmt::Debug = &self.engine_source;
        let (backend, program) = match &self.regex {
            Matcher::Standard(_) => ("regex-automata meta", source),
            #[cfg(target_os = "linux")]
            Matcher::Locale(locale) => (
                "regex-automata meta, LC_CTYPE at search time",
                locale.program(),
            ),
            Matcher::Lookahead(_) => ("fancy-regex", source),
            Matcher::Guarded(_) => ("progress-aware PikeVM", source),
            Matcher::Asserted(_) => ("assertion-aware Thompson NFA", source),
        };
        let kind = if self.bytes { "bytes" } else { "str" };
        write!(
            output,
            "Imre regex ({backend}, {kind}, flags={:#x}, groups={})\n  program {:?}",
            self.flags,
            self.group_count(),
            program,
        )
        .unwrap();
        for (slot, guard) in &self.assertions {
            write!(
                output,
                "\n  assertion {slot} {:?} width={} {:?}",
                guard.kind, guard.width, guard.program,
            )
            .unwrap();
        }
        if let Some(surrogates) = &self.surrogate_literals {
            output.push_str("\n  surrogate matcher\n");
            surrogates.debug_program(output);
        }
    }

    pub(in crate::dynamic) fn group_count(&self) -> usize {
        let info = self.regex.group_info();
        info.to_index(PatternID::ZERO, "imre.eol")
            .unwrap_or_else(|| info.group_len(PatternID::ZERO))
            - 1
    }

    /// Map public capture names to their one-based indices in pattern order.
    pub(in crate::dynamic) fn group_index(&self, ctx: &mut Context) -> Result<Id> {
        let info = self.regex.group_info();
        let mut fields = Vec::new();
        for index in 1..=self.group_count() {
            if let Some(name) = info.to_name(PatternID::ZERO, index) {
                fields.push((ctx.text(name.to_owned()), ctx.int(index as i64)));
            }
        }
        if fields.is_empty() {
            return Ok(ctx.dict(fields));
        }
        let mapping = ctx.dict(fields);
        ctx.mapping_proxy(mapping)
    }

    fn last_capture(&self, text: &[u8], caps: &Captures) -> LastCapture {
        let mut groups = (1..=self.group_count()).filter(|&i| caps.get_group(i).is_some());
        let first = groups.next();
        if groups.next().is_none() {
            return LastCapture::Known(first);
        }
        let order = self.close_order.get_or_init(|| match &self.regex {
            Matcher::Lookahead(matcher) => matcher.close_order(),
            Matcher::Asserted(_) => None, // The NFA maps assertion slots to public groups.
            _ => split::single_visit_order(self),
        });
        let winner = if let Some(order) = order {
            order
                .iter()
                .rev()
                .copied()
                .find(|&i| caps.get_group(i).is_some())
        } else if matches!(&self.regex, Matcher::Lookahead(_)) {
            None
        } else {
            split::last_close(self, text, caps)
        };
        match winner {
            Some(index) => LastCapture::Known(Some(index)),
            None => LastCapture::Unavailable,
        }
    }

    fn match_error(&self, ctx: &mut Context, message: String) -> Error {
        if matches!(self.regex, Matcher::Lookahead(_)) {
            Error::new("RuntimeError", message)
        } else {
            pattern_error(ctx, self.source, self.flags, message)
        }
    }

    fn engine_flags(&self) -> i64 {
        if self.source == NONE && self.flags & UNICODE == 0 {
            self.flags | ASCII
        } else {
            self.flags
        }
    }

    // CPython may match an empty alternative with pos past endpos, while a
    // zero-minimum repeat can fail there. Slicing to an empty haystack loses that distinction.
    fn check_reversed_match(&self) -> Result<()> {
        let consumes_input = match &self.regex {
            Matcher::Lookahead(matcher) => matcher.requires_input,
            _ => regex_automata::util::syntax::parse_with(
                &self.engine_source,
                &syntax(self.bytes, self.engine_flags()),
            )
            .is_ok_and(|hir| hir.properties().minimum_len() != Some(0)),
        };
        if consumes_input {
            return Ok(());
        }
        Err(Error::new(
            "NotImplementedError",
            "zero-width matching with pos past endpos is unsupported",
        ))
    }

    fn matcher(&self, full: bool) -> std::result::Result<&Matcher, String> {
        if !full {
            return Ok(&self.regex);
        }
        // The end assertion must participate in matching: a|ab must fully match "ab".
        self.full
            .get_or_init(|| {
                // For full matches only the true end of the input can satisfy a terminal dollar.
                let source = self
                    .engine_source
                    .strip_suffix(imre_regex::FINAL_NEWLINE_ANCHOR)
                    .unwrap_or(&self.engine_source);
                let full = format!(r"(?:{source})\z");
                #[cfg(target_os = "linux")]
                if let Matcher::Locale(matcher) = &self.regex {
                    return Ok(Matcher::Locale(Box::new(matcher.full())));
                }
                if let Matcher::Lookahead(matcher) = &self.regex {
                    return matcher
                        .with_source(&full)
                        .map(|m| Matcher::Lookahead(Box::new(m)));
                }
                build_regex(&full, self.bytes, self.engine_flags(), &self.assertions)
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

fn syntax(bytes: bool, flags: i64) -> regex_automata::util::syntax::Config {
    regex_automata::util::syntax::Config::new()
        .case_insensitive(flags & IGNORECASE != 0 && (bytes || flags & ASCII == 0))
        .multi_line(flags & MULTILINE != 0)
        .dot_matches_new_line(flags & DOTALL != 0)
        .unicode(!bytes)
        .utf8(!bytes)
}

fn build_regex(
    source: &str,
    bytes: bool,
    flags: i64,
    assertions: &[(String, Guard)],
) -> std::result::Result<Matcher, String> {
    let syntax = syntax(bytes, flags);
    let mut builder = Regex::builder();
    builder
        .syntax(syntax)
        .configure(Regex::config().utf8_empty(!bytes));
    let hir = match regex_automata::util::syntax::parse_with(source, &syntax) {
        Ok(hir) => hir,
        Err(err) if !assertions.is_empty() => return Err(err.to_string()),
        Err(_) => {
            // Keep the backend's usual diagnostic if parsing fails.
            return builder
                .build(source)
                .map(Matcher::Standard)
                .map_err(|err| err.to_string());
        }
    };
    let regex = builder
        .build_from_hir(&hir)
        .map_err(|err| err.to_string())?;
    Matcher::new(regex, hir, !bytes, assertions)
}

/// Only Python's redundant positive Unicode mode can fall back to the raw engine.
fn raw_unicode_compatible(source: &str, ascii: bool, boundaries: &[usize]) -> bool {
    use regex_syntax::ast::{Ast, Flag, parse::Parser};

    let Ok(ast) = Parser::new().parse(source) else {
        // The ordinary builder reports its syntax error if translation also fails.
        return true;
    };
    let mut nodes = vec![&ast];
    while let Some(node) = nodes.pop() {
        let flags = match node {
            Ast::Group(group) => {
                nodes.push(&group.ast);
                group.flags()
            }
            Ast::Flags(set) => Some(&set.flags),
            Ast::Repetition(rep) => {
                nodes.push(&rep.ast);
                None
            }
            Ast::Concat(concat) => {
                nodes.extend(&concat.asts);
                None
            }
            Ast::Alternation(alt) => {
                nodes.extend(&alt.asts);
                None
            }
            _ => None,
        };
        if let Some(unicode) = flags.and_then(|f| f.flag_state(Flag::Unicode))
            && (!unicode || ascii)
            && boundaries.binary_search(&node.span().start.offset).is_err()
        {
            return false;
        }
    }
    true
}

// Python treats the first character after '[' or '[^' as a set member. A second
// hyphen can then introduce a range. Leave later ambiguous operators alone.
fn literal_operator_start(mut input: std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let Some(first @ ('-' | '&' | '|' | '~')) = input.next() else {
        return false;
    };
    if input.peek() != Some(&first) {
        return false;
    }
    while let Some(c) = input.next() {
        match c {
            ']' => return true,
            '\\' => {
                input.next();
            }
            '-' | '&' | '|' | '~' if input.peek() == Some(&c) => return false,
            _ => {}
        }
    }
    false
}

// Python uses str.isalnum() plus "_" for \w and includes four additional
// ASCII separators in Unicode whitespace. The nested classes also work
// when these escapes occur inside a bracketed class in the input.
fn raw_pattern(pattern: &str, ascii: bool, flags: i64) -> Option<(String, bool, bool)> {
    if flags & VERBOSE != 0 {
        return None;
    }
    let check_unicode =
        pattern.contains("(?") && pattern.contains('u') && (ascii || pattern.contains('-'));
    let mut boundaries = Vec::new();
    let mut prefer_translation = false;
    let mut prefer_backtracking = false;
    let mut result = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        if c != '\\' {
            // The Python translator keeps a backtrackable prefix inside its ordered alternative.
            prefer_translation |= c == '|' && !in_class;
            if c == '(' && !in_class && chars.peek() == Some(&'?') {
                let mut group = chars.clone();
                group.next();
                prefer_translation |= group.peek() == Some(&'#');
                let mut prefer_python_flags = false;
                while let Some(flag) = group.next_if(|c| "aiLmsux-".contains(*c)) {
                    prefer_python_flags |= matches!(flag, 'a' | 'u' | 'i' | 'x');
                }
                if group.peek() == Some(&':') {
                    prefer_translation |= prefer_python_flags;
                }
            }
            // The regular backend reads a possessive modifier as another repetition.
            // Ask the Python tokenizer to bind it to its operand.
            prefer_backtracking |=
                !in_class && matches!(c, '*' | '+' | '?') && chars.peek() == Some(&'+');
            if c == '{' && !in_class {
                if !imre_regex::starts_counted_repeat(chars.clone()) {
                    prefer_translation = true;
                } else {
                    let mut suffix = chars
                        .clone()
                        .skip_while(|c| c.is_ascii_digit() || *c == ',');
                    prefer_backtracking |= suffix.next() == Some('}') && suffix.next() == Some('+');
                    if chars.peek() == Some(&',') {
                        // Python treats an omitted lower bound as zero, including in {,}.
                        result.push_str("{0");
                        continue;
                    }
                }
            }
            if c == '$' && !in_class && flags & MULTILINE == 0 {
                if chars.peek().is_none() {
                    result.push_str(imre_regex::FINAL_NEWLINE_ANCHOR);
                    continue;
                }
                // Inside a pattern, the final newline must remain available to the suffix.
                prefer_backtracking = true;
            }
            if c == '[' && !in_class {
                in_class = true;
                result.push('[');
                // Leave the warning-producing leading "[[" to the existing parser.
                if chars.next_if_eq(&'[').is_some() {
                    result.push('[');
                    continue;
                }
                if chars.next_if_eq(&'^').is_some() {
                    result.push('^');
                }
                // A closing bracket in the first position is a literal member.
                if chars.next_if_eq(&']').is_some() {
                    result.push(']');
                } else if literal_operator_start(chars.clone()) {
                    result.push('\\');
                }
                continue;
            }
            if c == '[' && in_class {
                // Python treats this class member as a scalar, including in ranges.
                result.push('\\');
            } else if c == ']' {
                in_class = false;
            }
            result.push(c);
            continue;
        }
        let escaped = chars.next();
        // Only the Python tokenizer knows the class context across verbose comments.
        prefer_translation |= matches!(escaped, Some('0'..='7'));
        if ascii && let Some(class) = escaped.and_then(imre_regex::ascii_class) {
            result.push_str(class);
            continue;
        }
        match escaped {
            Some('b') if in_class => result.push_str(r"\x08"),
            Some(boundary @ ('b' | 'B')) if ascii && !in_class => {
                if check_unicode {
                    boundaries.push(result.len());
                }
                result.push_str(if boundary == 'b' {
                    r"(?-u:\b)"
                } else {
                    r"(?-u:\B)"
                });
            }
            Some('w') => result.push_str(r"[\p{L}\p{N}_]"),
            Some('W') => result.push_str(r"[^\p{L}\p{N}_]"),
            Some('s') => result.push_str(r"[\s\x1c-\x1f]"),
            Some('S') => result.push_str(r"[^\s\x1c-\x1f]"),
            Some('Z') => result.push_str(r"\z"),
            escaped => {
                result.push('\\');
                if let Some(c) = escaped {
                    result.push(c);
                }
            }
        }
    }
    if check_unicode && !raw_unicode_compatible(&result, ascii, &boundaries) {
        return None;
    }
    Some((result, prefer_translation, prefer_backtracking))
}

/// Report offsets in the user's spelling before byte escaping, case folding or verbose translation.
fn syntax_diagnostic(ctx: &Context, source: Id, flags: i64) -> Option<(String, usize)> {
    use regex_syntax::ast::{ErrorKind, parse::ParserBuilder};

    let text: Cow<'_, str> = match ctx.heap.get(ctx.scalar_value(source)) {
        // Preserve one parser position per original Python code point.
        Value::SurrogateStr(points) => Cow::Owned(
            points
                .iter()
                .map(|&p| char::from_u32(p).unwrap_or('\u{fffd}'))
                .collect(),
        ),
        Value::Bytes(bytes) => Cow::Owned(bytes.iter().copied().map(char::from).collect()),
        _ => Cow::Borrowed(ctx.str_value(source).ok()?),
    };
    let bytes = matches!(ctx.heap.get(ctx.scalar_value(source)), Value::Bytes(_));
    let points = match ctx.heap.get(ctx.scalar_value(source)) {
        Value::SurrogateStr(points) => Some(points.as_slice()),
        _ => None,
    };
    let normalized = match escapes::normalize(&text, bytes, flags, true, points) {
        Ok(normalized) => normalized,
        Err(error) => return Some(error),
    };
    let text = normalized
        .as_ref()
        .map_or(text.as_ref(), |names| names.text.as_str());
    let position = |at| normalized.as_ref().map_or(at, |names| names.position(at));
    let parsed = ParserBuilder::new()
        .ignore_whitespace(flags & VERBOSE != 0)
        .build()
        .parse(text);
    if let Some((message, at)) = group_names::diagnostic(text, bytes, points, position, &parsed) {
        return Some((message, position(at)));
    }
    if let Some((msg, at)) = imre_regex::repeat_diagnostic(text, flags & VERBOSE != 0) {
        return Some((msg.to_owned(), position(text[..at].chars().count())));
    }
    let error = parsed.err()?;
    let message = match error.kind() {
        ErrorKind::ClassUnclosed => "unterminated character set",
        ErrorKind::GroupUnclosed => "missing ), unterminated subpattern",
        ErrorKind::GroupUnopened => "unbalanced parenthesis",
        ErrorKind::RepetitionMissing => "nothing to repeat",
        ErrorKind::EscapeUnexpectedEof => "bad escape (end of pattern)",
        _ => return None,
    };
    Some((
        message.into(),
        position(text[..error.span().start.offset].chars().count()),
    ))
}

/// Construct the public exception with the original pattern and the engine's diagnostic.
fn pattern_error(ctx: &mut Context, source: Id, flags: i64, message: String) -> Error {
    let (message, position) = match syntax_diagnostic(ctx, source, flags) {
        Some((message, position)) => (message, Some(position)),
        None => (message, None),
    };
    pattern_error_at(ctx, source, message, position)
}

fn pattern_error_at(
    ctx: &mut Context,
    source: Id,
    message: String,
    position: Option<usize>,
) -> Error {
    let Some(&class) = ctx.classes.get("re.PatternError") else {
        return Error::new("re.PatternError", message);
    };
    let message = ctx.text(message);
    let position = position.map_or(NONE, |pos| ctx.int(pos as i64));
    match ctx.call(class, vec![message, source, position], Vec::new()) {
        Ok(id) => match ctx.heap.get(id) {
            Value::Raised(error) => {
                let mut error = error.clone();
                error.tracing().instance = Some(id);
                error
            }
            _ => Error::new("TypeError", "exceptions must derive from BaseException"),
        },
        Err(error) => error,
    }
}

/// Repeat a mandatory literal capture when its spelling compares like the captured text.
fn repeated_literal_capture(pattern: &[u8], flags: i64, bytes: bool) -> Option<Vec<u8>> {
    if flags & VERBOSE != 0 || flags & (IGNORECASE | LOCALE) == IGNORECASE | LOCALE {
        return None;
    }
    let plain = |s: &[u8]| !s.iter().any(|b| br"\.^$*+?{}[]|()".contains(b));
    let mut groups = Vec::new();
    let mut rest = pattern;
    while let Some(capture) = rest.strip_prefix(b"(") {
        if capture.starts_with(b"?P=") {
            break;
        }
        let (name, body) = if let Some(named) = capture.strip_prefix(b"?P<") {
            let end = named.iter().position(|&b| b == b'>')?;
            let name = std::str::from_utf8(&named[..end]).ok()?;
            if !group_names::valid(name, bytes) {
                return None;
            }
            (Some(name.as_bytes()), &named[end + 1..])
        } else {
            (None, capture)
        };
        let close = body.iter().position(|&b| b == b')')?;
        let literal = &body[..close];
        if !plain(literal) {
            return None;
        }
        groups.push((name, literal));
        rest = &body[close + 1..];
    }
    let (literal, tail) = if let Some(digits) = rest.strip_prefix(br"\") {
        let [first @ b'1'..=b'9', tail @ ..] = digits else {
            return None;
        };
        // Three octal digits are a character escape, even when that capture exists.
        if matches!(digits, [b'0'..=b'7', b'0'..=b'7', b'0'..=b'7', ..]) {
            return None;
        }
        let mut number = usize::from(first - b'0');
        let tail = if let [second @ b'0'..=b'9', tail @ ..] = tail {
            number = number * 10 + usize::from(second - b'0');
            tail
        } else {
            tail
        };
        (groups.get(number - 1)?.1, tail)
    } else {
        let name = rest.strip_prefix(b"(?P=")?.strip_suffix(b")")?;
        (groups.iter().find(|g| g.0 == Some(name))?.1, &[][..])
    };
    if literal.is_empty() || !plain(tail) {
        return None;
    }
    // Unicode literals match a wider class than a backreference's lowercase comparison.
    // Of the ASCII literals, I/i admit dotless i and S/s admit long s.
    if !bytes
        && flags & (ASCII | IGNORECASE) == IGNORECASE
        && literal
            .iter()
            .any(|b| !b.is_ascii() || matches!(b, b'I' | b'i' | b'S' | b's'))
    {
        return None;
    }
    Some([&pattern[..pattern.len() - rest.len()], literal, tail].concat())
}

fn locale_dependent(source: &str, flags: i64) -> bool {
    flags & LOCALE != 0
        && (flags & IGNORECASE != 0
            || source
                .as_bytes()
                .windows(2)
                .any(|pair| matches!(pair, br"\w" | br"\W" | br"\b" | br"\B")))
}

fn alphabet(bytes: bool, ascii: bool, flags: i64) -> imre_regex::Alphabet {
    use imre_regex::Alphabet;
    if bytes {
        Alphabet::Bytes
    } else if ascii && flags & IGNORECASE != 0 {
        Alphabet::AsciiIgnoreCase
    } else if ascii {
        Alphabet::Ascii
    } else if flags & IGNORECASE != 0 {
        Alphabet::UnicodeIgnoreCase
    } else {
        Alphabet::Unicode
    }
}

fn compiled(ctx: &mut Context, id: Id, flags: Option<Id>) -> Result<Id> {
    let module = ctx.module("re")?;
    match ctx.lookup_attr(module, "_compile_cached") {
        Ok(cache) => {
            let raw = ctx.builtin("re._compile_uncached");
            let flags = flags.unwrap_or_else(|| ctx.int(0));
            ctx.call(cache, vec![raw, id, flags], Vec::new())
        }
        // Computed-only imports can expose the intrinsic module without its Python helpers.
        Err(error) if ctx.is_attribute_error(&error) => compile_uncached(ctx, id, flags),
        Err(error) => Err(error),
    }
}

fn pattern_spelling<'a>(
    ctx: &'a Context,
    id: Id,
    projections: &'a Option<(String, String)>,
) -> Result<&'a [u8]> {
    if let Some((_, encoded)) = projections {
        Ok(encoded.as_bytes())
    } else if let Value::Bytes(value) = ctx.heap.get(ctx.scalar_value(id)) {
        Ok(value.as_slice())
    } else {
        Ok(ctx.str_value(id)?.as_bytes())
    }
}

fn warn_class(ctx: &mut Context, kind: &str, position: usize) -> Result<()> {
    // Python wrappers such as _compile_cached and sub add internal re frames.
    let mut level = 1;
    while let Some((_, scope)) = ctx.heap.warning_frame(level) {
        let globals = ctx.module_globals(scope)?;
        if ctx
            .dict_named_item(globals, "__name__")
            .and_then(|id| ctx.str_value(id).ok())
            != Some("re")
        {
            break;
        }
        level += 1;
    }
    ctx.emit_builtin_warning(
        crate::objects::exceptions::Kind::FutureWarning,
        &format!("Possible {kind} at position {position}"),
        level,
    )
}

fn may_warn_class(source: &[u8]) -> bool {
    source.contains(&b'[')
        && source
            .windows(2)
            .any(|p| matches!(p, b"[[" | b"--" | b"&&" | b"||" | b"~~"))
}

fn quote_class_members(
    source: &str,
    flags: i64,
    bytes: bool,
    mut warn: impl FnMut(&str, usize) -> Result<()>,
) -> Result<Option<String>> {
    let mut emitted = Ok(());
    let mut warned = false;
    let literals = imre_regex::visit_class_warnings(
        source,
        flags & VERBOSE != 0,
        alphabet(bytes, bytes || flags & ASCII != 0, flags),
        flags & DOTALL != 0,
        flags::scoped_verbose,
        group_names::valid,
        |(kind, pos)| {
            warned = true;
            emitted = warn(kind, pos);
            emitted.is_ok()
        },
    );
    emitted?;
    if !warned {
        return Ok(None);
    }
    // Escape literal class punctuation, retaining range separators, comment
    // layout, and the grammar of the original pattern outside its classes.
    let mut positions = literals.into_iter().peekable();
    let mut result = String::with_capacity(source.len());
    for (i, c) in source.chars().enumerate() {
        if positions.next_if_eq(&i).is_some() {
            result.push('\\');
        }
        result.push(c);
    }
    Ok(Some(result))
}

fn compile_uncached(ctx: &mut Context, id: Id, flags: Option<Id>) -> Result<Id> {
    let number = flags
        .map(|value| ctx.as_int(value))
        .transpose()?
        .unwrap_or_else(|| 0.into());
    let integer = number.to_i64();
    if matches!(ctx.heap.get(id), Value::RegexPattern(_)) {
        if integer != Some(0) {
            return Err(Error::new(
                "ValueError",
                "cannot process flags argument with a compiled pattern",
            ));
        }
        return Ok(id);
    }
    // Parse with the known switches even if the eventual C int conversion will fail.
    let flags = integer.unwrap_or_else(|| {
        (number.big().as_ref() & num_bigint::BigInt::from(KNOWN_FLAGS))
            .to_i64()
            .unwrap()
    });
    let debug = flags & DEBUG;
    let unknown = flags & !KNOWN_FLAGS;
    let flags = flags & KNOWN_FLAGS & !DEBUG;
    let scalar = ctx.heap.get(ctx.scalar_value(id));
    let bytes = matches!(scalar, Value::Bytes(_));
    let projections = if let Value::SurrogateStr(points) = scalar {
        match surrogate::literal_patterns(points) {
            Ok(pair) => Some(pair),
            Err(error) => {
                check_flags(flags, false)?;
                return Err(error);
            }
        }
    } else if let Value::Str(text) = scalar {
        surrogate::class_patterns(text)
    } else {
        None
    };
    // The encoded subject projection retains one scalar per original code point.
    // Check placement and source positions there before compiling its scalar-only twin.
    let spelling = pattern_spelling(ctx, id, &projections)?;
    let parsed = flags::parse(spelling, flags, bytes);
    let body = &spelling[parsed.skip..];
    let quoted = if may_warn_class(body) {
        let start = flags::original_spelling(&spelling[..parsed.skip], bytes)
            .chars()
            .count();
        // Warning filters may execute Python and allocate on the heap.
        let source = flags::original_spelling(body, bytes).into_owned();
        quote_class_members(&source, parsed.bits, bytes, |kind, pos| {
            warn_class(ctx, kind, start + pos)
        })?
        .map(|s| {
            if bytes {
                s.chars().map(|c| c as u8).collect()
            } else {
                s.into_bytes()
            }
        })
    } else {
        None
    };
    if let Some((msg, pos)) = parsed.fatal {
        return Err(pattern_error_at(ctx, id, msg.into(), Some(pos)));
    }
    let supplied_flags = flags;
    let inline_verbose = flags & VERBOSE == 0 && parsed.bits & VERBOSE != 0;
    let flags = parsed.bits;
    let spelling = &pattern_spelling(ctx, id, &projections)?[parsed.skip..];
    if let Some(bad) = &parsed.deferred
        && valid_prefix(id, &bad.prefix, flags, bytes, inline_verbose)
    {
        return Err(pattern_error_at(
            ctx,
            id,
            bad.error.0.clone(),
            Some(bad.error.1),
        ));
    }
    check_flags(flags, bytes)?;
    let pattern = match prepare(
        id,
        quoted.as_deref().unwrap_or(spelling),
        flags,
        bytes,
        inline_verbose,
    ) {
        Ok(pattern) => pattern,
        Err(message) => {
            let may_reference = spelling.windows(3).any(|w| w == b"(?(")
                || spelling.windows(4).any(|w| w == b"(?P=")
                || spelling
                    .windows(2)
                    .any(|w| w[0] == b'\\' && matches!(w[1], b'1'..=b'9'));
            if may_reference
                && let Some((msg, offset)) = imre_regex::group_reference_diagnostic(
                    &flags::original_spelling(spelling, bytes),
                    flags & VERBOSE != 0,
                    alphabet(bytes, bytes || flags & ASCII != 0, flags),
                    flags & DOTALL != 0,
                    flags::scoped_verbose,
                    group_names::valid,
                    crate::integer_text::decimal_digit_limit() as usize,
                    true,
                )
            {
                let original = pattern_spelling(ctx, id, &projections)?;
                let start = flags::original_spelling(&original[..parsed.skip], bytes)
                    .chars()
                    .count();
                return Err(pattern_error_at(ctx, id, msg, Some(start + offset)));
            }
            return Err(pattern_error(ctx, id, flags, message));
        }
    };
    if let Some(name) = pattern.invalid_group_name(spelling) {
        if let Some((message, position)) = syntax_diagnostic(ctx, id, flags) {
            return Err(pattern_error_at(ctx, id, message, Some(position)));
        }
        if name != "imre.eol" {
            return Err(pattern_error_at(
                ctx,
                id,
                group_names::bad_name(name, bytes),
                None,
            ));
        }
    }
    if let Some(bad) = parsed.deferred {
        return Err(pattern_error_at(ctx, id, bad.error.0, Some(bad.error.1)));
    }
    let mut pattern = if let Some((plain, _)) = projections {
        // A surrogate inside a leading comment has a different byte width in the
        // scalar-only program. Locate that program's own first significant byte.
        let plain_flags = flags::parse(plain.as_bytes(), supplied_flags, false);
        if let Some((msg, pos)) = plain_flags.fatal {
            return Err(pattern_error_at(ctx, id, msg.into(), Some(pos)));
        }
        let body = &plain[plain_flags.skip..];
        let scalar_quoted = if quoted.is_some() {
            // Scalar-only replacements may have changed the length of the prefix.
            // Filters already ran on the projection with the original positions.
            quote_class_members(body, flags, false, |_, _| Ok(()))?
        } else {
            None
        };
        let mut scalar_pattern = prepare(
            id,
            scalar_quoted.as_deref().unwrap_or(body).as_bytes(),
            flags,
            false,
            inline_verbose,
        )
        .map_err(|message| pattern_error(ctx, id, flags, message))?;
        scalar_pattern.surrogate_literals = Some(Box::new(pattern));
        scalar_pattern
    } else {
        pattern
    };
    if integer.is_none_or(|value| i32::try_from(value).is_err()) {
        return Err(Error::new(
            "OverflowError",
            "Python int too large to convert to C int",
        ));
    }
    pattern.flags |= unknown | debug;
    if debug != 0 {
        let mut output = String::new();
        pattern.debug_program(&mut output);
        let print = ctx.builtin("print");
        let text = ctx.text(output);
        ctx.call(print, vec![text], Vec::new())?;
    }
    Ok(ctx.alloc(Value::RegexPattern(Rc::new(pattern))))
}

fn check_flags(flags: i64, bytes: bool) -> Result<()> {
    if bytes && flags & UNICODE != 0 {
        return Err(Error::new(
            "ValueError",
            "cannot use UNICODE flag with a bytes pattern",
        ));
    }
    if !bytes && flags & LOCALE != 0 {
        return Err(Error::new(
            "ValueError",
            "cannot use LOCALE flag with a str pattern",
        ));
    }
    if bytes && flags & (LOCALE | ASCII) == LOCALE | ASCII {
        return Err(Error::new(
            "ValueError",
            "ASCII and LOCALE flags are incompatible",
        ));
    }
    if flags & ASCII != 0 && flags & UNICODE != 0 {
        return Err(Error::new(
            "ValueError",
            "ASCII and UNICODE flags are incompatible",
        ));
    }
    Ok(())
}

fn valid_prefix(id: Id, prefix: &[u8], flags: i64, bytes: bool, inline_verbose: bool) -> bool {
    if let Ok(pattern) = prepare(id, prefix, flags, bytes, inline_verbose) {
        return pattern.invalid_group_name(prefix).is_none();
    }
    // A conditional with no other backtracking feature can be valid Python syntax
    // even when there is no native matcher for the truncated expression.
    prefix.windows(3).any(|w| w == b"(?(") && valid_translated_prefix(prefix, flags, bytes)
}

// The raw regex backend accepts extra syntax such as Unicode property escapes.
fn valid_translated_prefix(prefix: &[u8], flags: i64, bytes: bool) -> bool {
    let spelling = if bytes {
        imre_regex::byte_pattern(prefix)
    } else {
        std::str::from_utf8(prefix).unwrap().to_owned()
    };
    let Ok((source, names)) = imre_regex::validation_source(
        &spelling,
        flags & VERBOSE != 0,
        alphabet(bytes, bytes || flags & ASCII != 0, flags),
        flags & DOTALL != 0,
    ) else {
        return false;
    };
    let mut seen = std::collections::HashSet::new();
    names
        .iter()
        .flatten()
        .all(|name| group_names::valid(name, bytes) && seen.insert(name))
        && fancy_regex::Regex::new(&source).is_ok()
}

fn prepare(
    id: Id,
    spelling: &[u8],
    flags: i64,
    bytes: bool,
    inline_verbose: bool,
) -> std::result::Result<Pattern, String> {
    let repeated = repeated_literal_capture(spelling, flags, bytes);
    let spelling = repeated.as_deref().unwrap_or(spelling);
    let source = if bytes {
        imre_regex::byte_pattern(spelling)
    } else {
        std::str::from_utf8(spelling)
            .expect("text pattern")
            .to_owned()
    };
    let source = match escapes::normalize(&source, bytes, flags, false, None) {
        Ok(Some(names)) => names.text,
        Ok(None) => source,
        Err((message, _)) => return Err(message),
    };
    let ascii = bytes || flags & ASCII != 0;
    // Byte escapes have a separate grammar. Unicode IGNORECASE must fold
    // the literal guard without extending Python's word-category membership.
    let literal_guards = !bytes && (flags & IGNORECASE == 0 || ascii);
    let translate = |spelling: &str| {
        imre_regex::translate_pattern_with_line_mode(
            spelling,
            flags & VERBOSE != 0,
            alphabet(bytes, ascii, flags),
            flags & DOTALL != 0,
            literal_guards,
            flags & MULTILINE != 0,
        )
        .map(|parsed| parsed.source)
    };
    let build = |source: String| {
        let matcher = build_regex(&source, bytes, flags, &[])?;
        Ok((source, matcher))
    };
    let backtrack = |error| lookahead::compile(&source, flags, bytes, error);
    // Keep the raw fallback for inline verbose patterns beyond the Python tokenizer's
    // current grammar. Its parser needs the flag in the spelling to interpret layout.
    let raw_verbose = inline_verbose.then(|| format!("(?x){source}"));
    let raw = match &raw_verbose {
        Some(pattern) => raw_pattern(pattern, ascii, flags & !VERBOSE),
        None => raw_pattern(&source, ascii, flags),
    };
    let prepared = if let Some((raw, prefer_translation, prefer_backtracking)) = raw {
        let original = || {
            if let Some((msg, _)) = imre_regex::repeat_diagnostic(&source, flags & VERBOSE != 0) {
                return Err(msg.to_owned());
            }
            let raw = imre_regex::preserve_zero_repeat_groups(raw)?;
            if flags & IGNORECASE != 0 && !bytes {
                build(imre_regex::expand_case(&raw, ascii)?)
            } else {
                build(raw)
            }
        };
        let committed = prefer_backtracking
            .then(|| backtrack(String::new()).ok())
            .flatten();
        match committed {
            Some((source, Matcher::Lookahead(matcher))) if matcher.total_close_order() => {
                Ok((source, Matcher::Lookahead(matcher)))
            }
            fallback => {
                // Preserve the regular matcher's winning close trace when a repeated
                // capture has no fixed syntax order. The committed VM can still
                // handle such a pattern when the regular engine cannot compile it.
                let regular = if prefer_translation || inline_verbose {
                    translate(&source)
                        .and_then(build)
                        .or_else(|err| {
                            if fallback.is_some() {
                                Err(err)
                            } else {
                                backtrack(err)
                            }
                        })
                        .or_else(|_| original())
                } else {
                    original()
                };
                regular.or_else(|err| fallback.ok_or(err))
            }
        }
    } else {
        translate(&source).and_then(build)
    };
    let prepared = match prepared {
        Err(original)
            if !bytes
                && flags & VERBOSE == 0
                && (literal_guards && source.contains("(?!")
                    || flags & !UNICODE == 0 && source.contains("(?>")) =>
        {
            translate(&source).map_err(|_| original).and_then(build)
        }
        result => result,
    };
    let mut assertions = Vec::new();
    let (engine_source, regex) = prepared
        .or_else(|original| {
            let Some(extracted) = assertions::extract(&source, flags & VERBOSE != 0, bytes)? else {
                return Err(original);
            };
            let expected_groups = if extracted
                .guards
                .iter()
                .any(|(_, kind, _)| *kind != Kind::Ahead)
            {
                // Validate Python widths before HIR simplification can discard an impossible branch.
                let (_, groups) = imre_regex::lookahead_source(
                    &source,
                    flags & VERBOSE != 0,
                    alphabet(bytes, ascii, flags),
                    flags & DOTALL != 0,
                )?;
                if locale_dependent(&source, flags) {
                    return Err(original);
                }
                Some(groups)
            } else {
                None
            };
            let syntax = syntax(bytes, flags);
            let mut guards = Vec::new();
            for (name, kind, body) in extracted.guards {
                let mut translated = translate(&body)?;
                // Matching the guard only needs a boolean. A terminal dollar still admits a
                // final newline, but its private capture is unnecessary in this submatcher.
                if translated.ends_with(imre_regex::FINAL_NEWLINE_ANCHOR) {
                    if kind != Kind::Ahead {
                        // The optional consumed newline is not part of Python's lookbehind width.
                        return Err("terminal dollar in a bytes lookbehind is unsupported".into());
                    }
                    translated.truncate(translated.len() - imre_regex::FINAL_NEWLINE_ANCHOR.len());
                    translated.push_str(r"(?:\n)?\z");
                }
                let hir = regex_automata::util::syntax::parse_with(&translated, &syntax)
                    .map_err(|err| err.to_string())?;
                // Bound the work done each time the main NFA visits a zero-width assertion.
                let properties = hir.properties();
                let invalid = if kind == Kind::Ahead {
                    "lookahead requires a capture-free pattern of at most 256 bytes"
                } else {
                    "lookbehind requires a fixed-width pattern of at most 256 bytes"
                };
                let width = properties
                    .maximum_len()
                    .filter(|width| *width <= 256)
                    .ok_or(invalid)?;
                if kind != Kind::Ahead && properties.minimum_len() != Some(width) {
                    return Err("look-behind requires fixed-width pattern".into());
                }
                if kind != Kind::NegativeBehind && properties.explicit_captures_len() != 0 {
                    return Err(if kind == Kind::Ahead {
                        invalid
                    } else {
                        "positive lookbehind requires a capture-free pattern"
                    }
                    .into());
                }
                let guard = Regex::builder()
                    .configure(Regex::config().utf8_empty(!bytes))
                    .build_from_hir(&hir)
                    .map_err(|err| err.to_string())?;
                guards.push((
                    name,
                    Guard {
                        regex: guard,
                        program: hir.to_string(),
                        kind,
                        width,
                    },
                ));
            }
            let source = translate(&extracted.source)?;
            let matcher = build_regex(&source, bytes, flags, &guards)?;
            if let Some(expected) = expected_groups
                && !matcher
                    .group_info()
                    .pattern_names(PatternID::ZERO)
                    .filter(|name| *name != Some("imre.eol"))
                    .eq(expected.iter().map(Option::as_deref))
            {
                return Err("lookbehind would change the Python capture groups".into());
            }
            assertions = guards;
            Ok((source, matcher))
        })
        .or_else(backtrack)?;
    #[cfg(target_os = "linux")]
    let regex = if bytes
        && flags & (LOCALE | IGNORECASE) == LOCALE | IGNORECASE
        && let Some(locale) = locale::LocaleMatcher::parse(&source, flags, regex.group_info())
    {
        Matcher::Locale(Box::new(locale))
    } else {
        regex
    };
    Ok(Pattern {
        source: id,
        flags: if ascii { flags } else { flags | UNICODE },
        bytes,
        engine_source,
        surrogate_literals: None,
        regex,
        assertions,
        full: OnceCell::new(),
        captures: OnceCell::new(),
        full_captures: OnceCell::new(),
        close_order: OnceCell::new(),
        split_retry: OnceCell::new(),
    })
}

/// A fresh list of exact string or bytes fields for a runtime-supplied class.
/// Heap slots can move on allocation, so borrow the subject again for each
/// field. Only the returned payloads are copied.
fn plain_collection(ctx: &mut Context, pattern: Id, subject: Id, split: bool) -> Option<Id> {
    let (members, len) = match (ctx.heap.get(pattern), ctx.heap.get(subject)) {
        (Value::Str(pattern), Value::Str(s)) => {
            (imre_regex::plain_run(pattern.as_bytes())?, s.len())
        }
        (Value::Bytes(pattern), Value::Bytes(b)) => (imre_regex::plain_run(pattern)?, b.len()),
        _ => return None,
    };
    let mut runs = imre_regex::ByteRuns::new(&members, len);
    let mark = ctx.heap.temporary_mark();
    let mut fields = Vec::new();
    let mut cursor = 0;
    loop {
        let data = match ctx.heap.get(subject) {
            Value::Str(s) => s.as_bytes(),
            Value::Bytes(b) => b,
            _ => unreachable!(),
        };
        let next = runs.next(data);
        let span = match &next {
            Some(run) if split => cursor..run.start,
            Some(run) => run.clone(),
            None if split => cursor..data.len(),
            None => break,
        };
        let field = if span.len() == data.len() {
            subject
        } else {
            match ctx.heap.get(subject) {
                Value::Str(s) => {
                    let part = crate::Str::new(&s[span]);
                    ctx.heap.owned_text(part)
                }
                Value::Bytes(b) => {
                    let part = b[span].to_vec();
                    ctx.heap.bytes_from_data(part)
                }
                _ => unreachable!(),
            }
        };
        fields.push(field);
        let Some(run) = next else { break };
        cursor = run.end;
    }
    let result = ctx.alloc(Value::List(fields));
    ctx.heap.keep_call_result(mark, result);
    Some(result)
}

fn omitted_or_zero(ctx: &Context, value: Option<Id>) -> bool {
    value.is_none_or(|id| matches!(ctx.heap.get(id), Value::Int(n) if n.as_small() == Some(0)))
}

pub(super) fn module_call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let missing = |arg: &str| {
        Error::new(
            "TypeError",
            format!("{name} missing required argument '{arg}'"),
        )
    };
    if name == "re._compile_scanner" {
        let [lexicon, flags] = bind(name, args, kwargs, ["lexicon", "flags"])?;
        return scanner::compile_lexicon(ctx, lexicon.ok_or_else(|| missing("lexicon"))?, flags);
    }
    if name == "re._compile_uncached" {
        return compile_uncached(ctx, argument(args, 0)?, args.get(1).copied());
    }
    if name == "re.compile" {
        let [pattern, flags] = bind(name, args, kwargs, ["pattern", "flags"])?;
        return compiled(ctx, pattern.ok_or_else(|| missing("pattern"))?, flags);
    }
    if name == "re.split" {
        let [pattern, string, maxsplit, flags] = bind(
            name,
            args,
            kwargs,
            ["pattern", "string", "maxsplit", "flags"],
        )?;
        if let (Some(pattern), Some(string)) = (pattern, string)
            && omitted_or_zero(ctx, flags)
            && omitted_or_zero(ctx, maxsplit)
            && let Some(fields) = plain_collection(ctx, pattern, string, true)
        {
            return Ok(fields);
        }
        let pattern = compiled(ctx, pattern.ok_or_else(|| missing("pattern"))?, flags)?;
        return split(
            ctx,
            pattern,
            string.ok_or_else(|| missing("string"))?,
            maxsplit,
        );
    }
    let [pattern, string, flags] = bind(name, args, kwargs, ["pattern", "string", "flags"])?;
    let string = string.ok_or_else(|| missing("string"))?;
    if name == "re.findall"
        && omitted_or_zero(ctx, flags)
        && let Some(pattern) = pattern
        && let Some(fields) = plain_collection(ctx, pattern, string, false)
    {
        return Ok(fields);
    }
    let pattern = compiled(ctx, pattern.ok_or_else(|| missing("pattern"))?, flags)?;
    match name.strip_prefix("re.") {
        Some(method @ ("match" | "fullmatch" | "search" | "finditer" | "findall")) => {
            search_pattern(ctx, method, pattern, string, None, None)
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("no regex operation {name}"),
        )),
    }
}

/// Hide the terminal dollar's synthetic capture and its consumed newline.
fn group_spans(caps: &Captures) -> impl Iterator<Item = Option<Span>> + '_ {
    let eol = caps.group_info().to_index(PatternID::ZERO, "imre.eol");
    (0..eol.unwrap_or(caps.group_len())).map(move |index| {
        let mut span = caps.get_group(index)?;
        if index == 0
            && let Some(newline) = eol.and_then(|i| caps.get_group(i))
        {
            span.end = newline.start;
        }
        Some(span)
    })
}

enum LastCapture {
    Known(Option<usize>),
    Unavailable,
}

pub(in crate::dynamic) struct Match {
    last: LastCapture,
    pub(in crate::dynamic) pattern: Id,
    pub(in crate::dynamic) subject: Id,
    spans: Vec<Option<Span>>,
    window: Span,
    pub(in crate::dynamic) cached_regs: OnceCell<Id>,
}

impl Match {
    fn pattern<'a>(&self, ctx: &'a Context) -> &'a Pattern {
        let Value::RegexPattern(pattern) = ctx.heap.get(self.pattern) else {
            unreachable!()
        };
        pattern
    }

    pub(in crate::dynamic) fn last_attribute(&self, ctx: &mut Context, named: bool) -> Result<Id> {
        let index = match self.last {
            LastCapture::Known(index) => index,
            LastCapture::Unavailable => {
                return Err(Error::new(
                    "NotImplementedError",
                    "capture close order is unavailable for this regular expression",
                ));
            }
        };
        let Some(index) = index else { return Ok(NONE) };
        if named {
            let name = self
                .pattern(ctx)
                .regex
                .group_info()
                .to_name(PatternID::ZERO, index)
                .map(str::to_owned);
            Ok(name.map_or(NONE, |name| ctx.text(name)))
        } else {
            Ok(ctx.int(index as i64))
        }
    }

    fn group_index(&self, ctx: &mut Context, group: Id) -> Result<usize> {
        let index = if let Some(number) = ctx.index_value(group)? {
            number.to_i64().and_then(|i| usize::try_from(i).ok())
        } else {
            let info = self.pattern(ctx).regex.group_info();
            ctx.str_value(group)
                .ok()
                .and_then(|name| info.to_index(PatternID::ZERO, name))
        };
        index
            .filter(|&i| i < self.spans.len())
            .ok_or_else(|| Error::new("IndexError", "no such group"))
    }

    /// Resolve a capture by its integer index or name, including nonparticipating groups.
    pub(in crate::dynamic) fn group_item(&self, ctx: &mut Context, key: Id) -> Result<Id> {
        let index = self.group_index(ctx, key)?;
        self.group(ctx, index, NONE)
    }

    fn group(&self, ctx: &mut Context, index: usize, default: Id) -> Result<Id> {
        let Some(span) = self.spans[index] else {
            return Ok(default);
        };
        if let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(self.subject)) {
            let value = crate::dynamic::copy_ids(&points[span.range()])?;
            return Ok(ctx.text_codepoints(value));
        }
        let value = group_value(self.captured_bytes(ctx, span)?, self.pattern(ctx).bytes);
        Ok(ctx.alloc(value))
    }

    fn captured_bytes<'a>(&self, ctx: &'a Context, span: Span) -> Result<&'a [u8]> {
        let text = subject_bytes(ctx, self.subject, self.pattern(ctx).bytes)?;
        // A bytearray may have changed size since the search.
        Ok(&text[span.start.min(text.len())..span.end.min(text.len())])
    }

    pub(in crate::dynamic) fn repr(&self, ctx: &Context) -> Result<String> {
        let span = self.spans[0].expect("a match owns group zero");
        let quoted =
            if let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(self.subject)) {
                super::super::operators::surrogate_repr(&points[span.range()])
            } else {
                let data = self.captured_bytes(ctx, span)?;
                if self.pattern(ctx).bytes {
                    let mut quoted = String::new();
                    crate::objects::write_bytes_repr(&mut quoted, data);
                    quoted
                } else {
                    let text = std::str::from_utf8(data).expect("capture is UTF-8");
                    crate::objects::exceptions::string_repr(text).into_string()
                }
            };
        let (start, end) = self.bounds(ctx, 0)?;
        Ok(format!(
            "<re.Match object; span=({start}, {end}), match={quoted:.50}>"
        ))
    }

    fn bounds(&self, ctx: &Context, index: usize) -> Result<(i64, i64)> {
        match self.spans[index] {
            Some(span) => self.span_bounds(ctx, span),
            None => Ok((-1, -1)),
        }
    }

    fn span_bounds(&self, ctx: &Context, span: Span) -> Result<(i64, i64)> {
        let scalar = ctx.scalar_value(self.subject);
        if self.pattern(ctx).bytes
            || ctx.heap.is_ascii_str(scalar)
            || matches!(ctx.heap.get(scalar), Value::SurrogateStr(_))
        {
            return Ok((span.start as i64, span.end as i64));
        }
        let position = |byte| {
            ctx.heap
                .string_character_offset(scalar, byte)
                .expect("regex spans end on UTF-8 boundaries") as i64
        };
        let (start, end) = (position(span.start), position(span.end));
        Ok((start, end))
    }

    pub(in crate::dynamic) fn window_position(&self, ctx: &mut Context, end: bool) -> Result<Id> {
        let offset = if end {
            self.window.end
        } else {
            self.window.start
        };
        let (position, _) = self.span_bounds(ctx, Span::from(offset..offset))?;
        Ok(ctx.int(position))
    }

    pub(in crate::dynamic) fn regs(&self, ctx: &mut Context) -> Result<Id> {
        if let Some(&regs) = self.cached_regs.get() {
            return Ok(regs);
        }
        let mut groups = Vec::new();
        for index in 0..self.spans.len() {
            let (start, end) = self.bounds(ctx, index)?;
            let start = ctx.int(start);
            let end = ctx.int(end);
            groups.push(ctx.alloc(Value::Tuple(vec![start, end])));
        }
        let regs = ctx.alloc(Value::Tuple(groups));
        Ok(*self.cached_regs.get_or_init(|| regs))
    }
}

fn matched(ctx: &Context, receiver: Id) -> Result<Rc<Match>> {
    match ctx.heap.get(receiver) {
        Value::RegexMatch(m) => Ok(Rc::clone(m)),
        _ => Err(Error::new("TypeError", "expected regex match")),
    }
}

/// Return the captures after group zero, retaining the caller's default for unmatched groups.
pub(super) fn match_groups(ctx: &mut Context, receiver: Id, default: Id) -> Result<Id> {
    let m = matched(ctx, receiver)?;
    let groups = (1..m.spans.len())
        .map(|i| m.group(ctx, i, default))
        .collect::<Result<_>>()?;
    Ok(ctx.alloc(Value::Tuple(groups)))
}

pub(super) fn match_groupdict(ctx: &mut Context, receiver: Id, default: Id) -> Result<Id> {
    let m = matched(ctx, receiver)?;
    let mut fields = Vec::new();
    for index in 1..m.spans.len() {
        if let Some(name) = m
            .pattern(ctx)
            .regex
            .group_info()
            .to_name(PatternID::ZERO, index)
        {
            let key = ctx.text(name.to_owned());
            fields.push((key, m.group(ctx, index, default)?));
        }
    }
    Ok(ctx.dict(fields))
}

fn group_value(data: &[u8], bytes: bool) -> Value {
    if bytes {
        Value::Bytes(data.to_vec())
    } else {
        Value::Str(
            std::str::from_utf8(data)
                .expect("text regex captures end at UTF-8 boundaries")
                .into(),
        )
    }
}

fn subject_bytes(ctx: &Context, id: Id, bytes: bool) -> Result<&[u8]> {
    let value = ctx.heap.get(ctx.scalar_value(id));
    if !matches!(value, Value::Str(_) | Value::SurrogateStr(_)) {
        ctx.check_buffer_exporter(id)?;
    }
    let bad_subject = |_| {
        let name = match ctx.heap.get(id) {
            Value::Native(_) => ctx
                .builtin_type_name(id)
                .and_then(|name| name.rsplit('.').next())
                .unwrap_or_else(|| ctx.type_name(id)),
            _ => ctx.type_name(id),
        };
        Error::new(
            "TypeError",
            format!(
                "expected string or bytes-like object, got '{}'",
                &name[..name.floor_char_boundary(200)]
            ),
        )
    };
    match value {
        Value::Bytes(_) | Value::ByteArray(_) | Value::Array(_) | Value::MemoryView { .. } => {
            // SRE acquires a simple buffer before checking the pattern's type.
            let data = ctx.native_buffer_bytes(id).map_err(bad_subject)?;
            if bytes {
                return Ok(data);
            }
            Err(Error::new(
                "TypeError",
                "cannot use a string pattern on a bytes-like object",
            ))
        }
        Value::Str(_) | Value::SurrogateStr(_) if bytes => Err(Error::new(
            "TypeError",
            "cannot use a bytes pattern on a string-like object",
        )),
        _ => ctx.str_value(id).map(str::as_bytes).map_err(bad_subject),
    }
}

/// The engine searches bytes, while matches on code-point strings index the original subject.
struct SearchSubject {
    id: Id,
    bytes_pattern: bool,
    projection: Option<surrogate::Projection>,
}

impl SearchSubject {
    fn for_pattern<'a>(
        ctx: &Context,
        pattern: &'a Pattern,
        id: Id,
        method: &str,
    ) -> Result<(&'a Pattern, Self)> {
        let mut subject = Self {
            id,
            bytes_pattern: pattern.bytes,
            projection: None,
        };
        if let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(id))
            && !pattern.bytes
        {
            if let Some(encoded) = &pattern.surrogate_literals {
                subject.projection = Some(surrogate::Projection::for_literals(method, points)?);
                return Ok((encoded, subject));
            }
            subject.projection = Some(surrogate::Projection::new(
                ctx.str_value(pattern.source).ok(),
                &pattern.regex,
                method,
                points,
            )?);
        }
        Ok((pattern, subject))
    }

    fn bytes<'a>(&'a self, ctx: &'a Context) -> Result<&'a [u8]> {
        match &self.projection {
            Some(p) => Ok(p.text.as_bytes()),
            None => subject_bytes(ctx, self.id, self.bytes_pattern),
        }
    }

    fn original(&self, span: Span) -> Span {
        self.projection.as_ref().map_or(span, |p| p.original(span))
    }

    fn slice(&self, ctx: &mut Context, span: Span) -> Result<Id> {
        if let Some(p) = &self.projection {
            let points = ctx.string_codepoints(self.id)?;
            let slice = crate::dynamic::copy_ids(&points[p.original(span).range()])?;
            return Ok(ctx.text_codepoints(slice));
        }
        let data = &subject_bytes(ctx, self.id, self.bytes_pattern)?[span.range()];
        Ok(ctx.alloc(group_value(data, self.bytes_pattern)))
    }
}

pub(super) fn search(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let receiver = argument(args, 0)?;
    let [string, pos, endpos] = crate::dynamic::builtins::bind_method_arguments(
        method,
        &args[1..],
        kwargs,
        ["string", "pos", "endpos"],
        3,
    )?;
    let string = string.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("{method}() missing required argument 'string' (pos 1)"),
        )
    })?;
    search_pattern(ctx, method, receiver, string, pos, endpos)
}

/// Convert the Python character bounds before searching the original UTF-8 text.
fn search_pattern(
    ctx: &mut Context,
    method: &str,
    receiver: Id,
    string: Id,
    pos: Option<Id>,
    endpos: Option<Id>,
) -> Result<Id> {
    let mut index = |arg| -> Result<usize> {
        let value = ctx
            .index_integer(arg)?
            .to_i64()
            .and_then(|v| isize::try_from(v).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?;
        Ok(usize::try_from(value).unwrap_or(0))
    };
    let start = pos.map(&mut index).transpose()?.unwrap_or(0);
    let end = endpos.map(index).transpose()?.unwrap_or(usize::MAX);
    let pattern = match ctx.heap.get(receiver) {
        Value::RegexPattern(pattern) => Rc::clone(pattern),
        _ => {
            return Err(Error::new(
                "TypeError",
                "expected compiled regular expression",
            ));
        }
    };
    if method == "findall"
        && pos.is_none()
        && endpos.is_none()
        && (pattern.flags == 0 || !pattern.bytes && pattern.flags == UNICODE)
        && let Some(fields) = plain_collection(ctx, pattern.source, string, false)
    {
        return Ok(fields);
    }
    let (pattern, subject) = SearchSubject::for_pattern(ctx, &pattern, string, method)?;
    let text = subject.bytes(ctx)?;
    let byte = |i: usize| {
        if pattern.bytes || i >= text.len() {
            i.min(text.len())
        } else if subject.projection.is_none() {
            ctx.heap
                .string_byte_offset(ctx.scalar_value(string), i)
                .unwrap_or(text.len())
        } else {
            std::str::from_utf8(text)
                .expect("text regex subject is UTF-8")
                .char_indices()
                .nth(i)
                .map_or(text.len(), |(offset, _)| offset)
        }
    };
    let (start, end) = (byte(start), byte(end));
    if method == "findall" {
        // Keep the prefix for anchors and word boundaries before pos.
        let text = &text[..end];
        let mut captures = pattern.regex.create_captures();
        return match split::next_span(pattern, text, start, None, &mut captures) {
            Ok(Some(first)) => {
                let snapshot = match &subject.projection {
                    Some(p) => Cow::Borrowed(&p.text.as_bytes()[..end]),
                    None => Cow::Owned(text.to_vec()),
                };
                findall(ctx, pattern, &subject, &snapshot, first, &mut captures)
            }
            Ok(None) => Ok(ctx.alloc(Value::List(Vec::new()))),
            Err(message) => Err(pattern.match_error(ctx, message)),
        };
    }
    if matches!(method, "scanner" | "finditer") {
        // The scanner exports its subject until destruction; a Match only retains it.
        let storage = ctx.view_storage_id(string);
        if matches!(
            ctx.heap.get(storage),
            Value::ByteArray(_) | Value::Array(_) | Value::MemoryView { .. }
        ) {
            ctx.heap.note_buffer_view(storage);
        }
        let scanner = ctx.alloc(Value::RegexScanner {
            pattern: receiver,
            subject: string,
            initial_start: start,
            start: Some(start),
            end,
            retry: false,
        });
        if method == "finditer" {
            let search = ctx.lookup_attr(scanner, "search")?;
            return Ok(ctx.alloc(Value::CallableIterator(Some([search, NONE]))));
        }
        return Ok(scanner);
    }
    if start > end {
        if method == "match" {
            pattern.check_reversed_match()?;
        }
        return Ok(NONE);
    }
    let full = method == "fullmatch";
    let matcher = match pattern.matcher(full) {
        Ok(matcher) => matcher,
        Err(message) => return Err(pattern_error(ctx, pattern.source, pattern.flags, message)),
    };
    let anchored = if method == "search" {
        Anchored::No
    } else {
        Anchored::Yes
    };
    let input = Input::new(&text[..end])
        .range(start..end)
        .anchored(anchored);
    let storage = if full {
        &pattern.full_captures
    } else {
        &pattern.captures
    };
    let mut captures = matcher.capture_buffer(storage).borrow_mut();
    matcher
        .captures(input, &mut captures)
        .map_err(|message| Error::new("RuntimeError", message))?;
    if !captures.is_match() {
        return Ok(NONE);
    }
    let last = pattern.last_capture(&text[..end], &captures);
    let spans = group_spans(&captures)
        .map(|span| span.map(|s| subject.original(s)))
        .collect();
    drop(captures);
    Ok(ctx.alloc(Value::RegexMatch(Rc::new(Match {
        last,
        pattern: receiver,
        subject: string,
        spans,
        window: subject.original(Span::from(start..end)),
        cached_regs: OnceCell::new(),
    }))))
}

pub(super) fn match_method(ctx: &mut Context, name: &str, args: &[Id]) -> Result<Id> {
    let m = matched(ctx, argument(args, 0)?)?;
    if name == "match.group" {
        if args.len() == 1 {
            return m.group(ctx, 0, NONE);
        }
        let mut values = Vec::with_capacity(args.len() - 1);
        for &group in &args[1..] {
            values.push(m.group_item(ctx, group)?);
        }
        return Ok(if values.len() == 1 {
            values[0]
        } else {
            ctx.alloc(Value::Tuple(values))
        });
    }
    let index = args.get(1).map_or(Ok(0), |&id| m.group_index(ctx, id))?;
    let (start, end) = m.bounds(ctx, index)?;
    Ok(match name {
        "match.start" => ctx.int(start),
        "match.end" => ctx.int(end),
        "match.span" => {
            let start = ctx.int(start);
            let end = ctx.int(end);
            ctx.alloc(Value::Tuple(vec![start, end]))
        }
        _ => unreachable!(),
    })
}

/// Collect the first match and the remaining captures from the bounded subject snapshot.
fn findall(
    ctx: &mut Context,
    pattern: &Pattern,
    subject: &SearchSubject,
    text: &[u8],
    mut span: Span,
    captures: &mut Captures,
) -> Result<Id> {
    let capture = |ctx: &mut Context, field: Option<Span>| {
        let span = field.unwrap_or_else(|| Span::from(0..0));
        if subject.projection.is_some() {
            subject.slice(ctx, span)
        } else {
            Ok(ctx.alloc(group_value(&text[span.range()], pattern.bytes)))
        }
    };
    let groups = pattern.group_count();
    let mut found = Vec::new();
    loop {
        let value = match groups {
            0 => capture(ctx, Some(span))?,
            1 => capture(ctx, captures.get_group(1))?,
            _ => {
                let values = (1..=groups)
                    .map(|i| capture(ctx, captures.get_group(i)))
                    .collect::<Result<_>>()?;
                ctx.alloc(Value::Tuple(values))
            }
        };
        found.push(value);
        let Some(next) = split::next_span(pattern, text, span.end, Some(span), captures)
            .map_err(|message| pattern.match_error(ctx, message))?
        else {
            break;
        };
        span = next;
    }
    Ok(ctx.alloc(Value::List(found)))
}
