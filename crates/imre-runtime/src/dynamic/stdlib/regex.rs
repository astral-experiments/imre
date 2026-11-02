use std::cell::OnceCell;
use std::fmt::Write;
use std::rc::Rc;

use regex_automata::{Anchored, Input, PatternID, Span, meta::Regex, util::captures::Captures};

use super::{Context, Error, Id, NONE, Result, Value, argument};
use crate::dynamic::builtins::bind;

mod assertions;
mod group_names;
mod lookahead;
mod progress;
mod scanner;
mod split;
mod sub;

pub(super) use scanner::scanner_next;

use progress::Matcher;
pub(super) use split::pattern_split;
use split::split;
pub(super) use sub::pattern_sub;

const IGNORECASE: i64 = 2;
const LOCALE: i64 = 4;
const MULTILINE: i64 = 8;
const DOTALL: i64 = 16;
const UNICODE: i64 = 32;
const VERBOSE: i64 = 64;
const ASCII: i64 = 256;

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
    ("DEBUG", "", 128),
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
    regex: Matcher,
    assertions: Vec<(String, Regex)>,
    full: OnceCell<std::result::Result<Matcher, String>>,
    split_retry: OnceCell<std::result::Result<regex_automata::nfa::thompson::NFA, String>>,
}

impl Pattern {
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
            let flags = names
                .into_iter()
                .map(|(name, _, _)| format!("re.{name}"))
                .collect::<Vec<_>>()
                .join("|");
            format!("re.compile({source}, {flags})")
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
        if regex_automata::util::syntax::parse_with(
            &self.engine_source,
            &syntax(self.bytes, self.engine_flags()),
        )
        .is_ok_and(|hir| hir.properties().minimum_len() != Some(0))
        {
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
                if let Matcher::Lookahead(matcher) = &self.regex {
                    return lookahead::Lookahead::new(&full, matcher.groups.clone())
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
    assertions: &[(String, Regex)],
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

/// Preserve byte literals outside ASCII in the regex parser's text input.
fn byte_pattern(bytes: &[u8]) -> String {
    let mut source = String::with_capacity(bytes.len());
    let mut escaped = false;
    for &b in bytes {
        if b.is_ascii() {
            source.push(char::from(b));
            escaped = b == b'\\' && !escaped;
        } else {
            if !escaped {
                source.push('\\');
            }
            write!(source, "x{b:02x}").unwrap();
            escaped = false;
        }
    }
    source
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

// Python uses str.isalnum() plus "_" for \w and includes four additional
// ASCII separators in Unicode whitespace. The nested classes also work
// when these escapes occur inside a bracketed class in the input.
fn raw_pattern(pattern: &str, ascii: bool, flags: i64) -> Option<(String, bool)> {
    if flags & VERBOSE != 0 {
        return None;
    }
    let check_unicode =
        pattern.contains("(?") && pattern.contains('u') && (ascii || pattern.contains('-'));
    let mut boundaries = Vec::new();
    let mut prefer_translation = false;
    let mut result = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        if c != '\\' {
            if c == '(' && !in_class && chars.peek() == Some(&'?') {
                let mut group = chars.clone();
                group.next();
                let mut prefer_python_flags = false;
                while let Some(flag) = group.next_if(|c| "aiLmsux-".contains(*c)) {
                    prefer_python_flags |= matches!(flag, 'a' | 'u' | 'i' | 'x');
                }
                if group.peek() == Some(&':') {
                    prefer_translation |= prefer_python_flags;
                }
            }
            if c == '{' && !in_class {
                if !imre_regex::starts_counted_repeat(chars.clone()) {
                    prefer_translation = true;
                } else if chars.peek() == Some(&',') {
                    // Python treats an omitted lower bound as zero, including in {,}.
                    result.push_str("{0");
                    continue;
                }
            }
            if c == '$' && !in_class && flags & MULTILINE == 0 && chars.peek().is_none() {
                result.push_str(imre_regex::FINAL_NEWLINE_ANCHOR);
                continue;
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
    Some((result, prefer_translation))
}

/// Report offsets in the user's spelling before byte escaping, case folding or verbose translation.
fn syntax_diagnostic(ctx: &Context, source: Id, flags: i64) -> Option<(String, usize)> {
    use regex_syntax::ast::{ErrorKind, parse::ParserBuilder};
    use std::borrow::Cow;

    let text: Cow<'_, str> = match ctx.heap.get(source) {
        Value::Bytes(bytes) => Cow::Owned(bytes.iter().copied().map(char::from).collect()),
        _ => Cow::Borrowed(ctx.str_value(source).ok()?),
    };
    let parsed = ParserBuilder::new()
        .ignore_whitespace(flags & VERBOSE != 0)
        .build()
        .parse(&text);
    if let Some(error) = group_names::diagnostic(
        &text,
        matches!(ctx.heap.get(source), Value::Bytes(_)),
        &parsed,
    ) {
        return Some(error);
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
        text[..error.span().start.offset].chars().count(),
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

/// A plain literal capture always participates and has exactly one possible spelling.
fn repeated_literal_capture(pattern: &[u8], flags: i64, bytes: bool) -> Option<(usize, usize)> {
    if flags & (IGNORECASE | LOCALE | VERBOSE) != 0 {
        return None;
    }
    let capture = pattern.strip_prefix(b"(")?;
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
    let reference = &body[close + 1..];
    if reference != br"\1"
        && !(name.is_some()
            && reference
                .strip_prefix(b"(?P=")
                .and_then(|r| r.strip_suffix(b")"))
                == name)
    {
        return None;
    }
    let literal = &body[..close];
    (!literal.is_empty() && !literal.iter().any(|b| br"\.^$*+?{}[]|()".contains(b)))
        .then_some((pattern.len() - body.len(), reference.len()))
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
    let flags = flags
        .map(|value| ctx.as_integer(value))
        .transpose()?
        .unwrap_or(0);
    if matches!(ctx.heap.get(id), Value::RegexPattern(_)) {
        if flags != 0 {
            return Err(Error::new(
                "ValueError",
                "cannot process flags argument with a compiled pattern",
            ));
        }
        return Ok(id);
    }
    let bytes = matches!(ctx.heap.get(id), Value::Bytes(_));
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
    if flags & !(IGNORECASE | LOCALE | MULTILINE | DOTALL | ASCII | UNICODE | VERBOSE) != 0 {
        return Err(Error::new(
            "NotImplementedError",
            "unsupported native regular expression flags",
        ));
    }
    let (mut source, spelling): (String, &[u8]) = match ctx.heap.get(id) {
        Value::Bytes(value) => (byte_pattern(value), value),
        _ => {
            let text = ctx.str_value(id)?;
            (text.to_owned(), text.as_bytes())
        }
    };
    if let Some((start, suffix)) = repeated_literal_capture(spelling, flags, bytes) {
        // Keep group 1 and repeat its engine spelling, including any escaped high bytes.
        source.truncate(source.len() - suffix);
        source.extend_from_within(start..source.len() - 1);
    }
    let ascii = bytes || flags & ASCII != 0;
    // Byte escapes have a separate grammar. Unicode IGNORECASE must fold
    // the literal guard without extending Python's word-category membership.
    let literal_guards = !bytes && (flags & IGNORECASE == 0 || ascii);
    let translate = |spelling: &str| {
        let mut translated = imre_regex::translate_pattern(
            spelling,
            flags & VERBOSE != 0,
            alphabet(bytes, ascii, flags),
            flags & DOTALL != 0,
            literal_guards,
        )?
        .source;
        if flags & MULTILINE != 0 && translated.ends_with(imre_regex::FINAL_NEWLINE_ANCHOR) {
            translated.truncate(translated.len() - imre_regex::FINAL_NEWLINE_ANCHOR.len());
            translated.push('$');
        }
        Ok(translated)
    };
    let build = |source: String| {
        let matcher = build_regex(&source, bytes, flags, &[])?;
        Ok((source, matcher))
    };
    let backtrack = |error| {
        if bytes {
            Err(error)
        } else {
            lookahead::compile(&source, flags, error)
        }
    };
    let global_verbose = source.starts_with("(?x)");
    let prepared = if let Some((raw, prefer_translation)) = raw_pattern(&source, ascii, flags) {
        let original = || {
            let raw = imre_regex::preserve_zero_repeat_groups(raw)?;
            if flags & IGNORECASE != 0 && !bytes {
                build(imre_regex::expand_case(&raw, ascii)?)
            } else {
                build(raw)
            }
        };
        if prefer_translation || global_verbose {
            // Python has its own verbose layout and literal-brace rules. Keep the
            // direct parser for features outside the subset translator.
            translate(&source)
                .and_then(build)
                .or_else(backtrack)
                .or_else(|_| original())
        } else {
            original()
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
            let Some(extracted) = assertions::extract(&source, flags & VERBOSE != 0)? else {
                return Err(original);
            };
            let syntax = syntax(bytes, flags);
            let mut guards = Vec::new();
            for (name, body) in extracted.guards {
                let mut translated = translate(&body)?;
                // Matching the guard only needs a boolean. A terminal dollar still admits a
                // final newline, but its private capture is unnecessary in this submatcher.
                if translated.ends_with(imre_regex::FINAL_NEWLINE_ANCHOR) {
                    translated.truncate(translated.len() - imre_regex::FINAL_NEWLINE_ANCHOR.len());
                    translated.push_str(r"(?:\n)?\z");
                }
                let hir = regex_automata::util::syntax::parse_with(&translated, &syntax)
                    .map_err(|err| err.to_string())?;
                // Bound the work done each time the main NFA visits a zero-width assertion.
                if hir
                    .properties()
                    .maximum_len()
                    .is_none_or(|width| width > 256)
                    || hir.properties().explicit_captures_len() != 0
                {
                    return Err(
                        "lookahead requires a capture-free pattern of at most 256 bytes".into(),
                    );
                }
                let guard = Regex::builder()
                    .configure(Regex::config().utf8_empty(!bytes))
                    .build_from_hir(&hir)
                    .map_err(|err| err.to_string())?;
                guards.push((name, guard));
            }
            let source = translate(&extracted.source)?;
            let matcher = build_regex(&source, bytes, flags, &guards)?;
            assertions = guards;
            Ok((source, matcher))
        })
        .or_else(backtrack)
        .map_err(|message| pattern_error(ctx, id, flags, message))?;
    // GroupInfo contains the synthetic final-newline capture as well as user names.
    let names = regex.group_info().pattern_names(PatternID::ZERO).flatten();
    if let Some(name) = names
        .filter(|name| *name != "imre.eol" || source.contains("(?P<imre.eol>"))
        .find(|name| !group_names::valid(name, bytes))
    {
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
    let flags = if global_verbose {
        flags | VERBOSE
    } else {
        flags
    };
    Ok(ctx.alloc(Value::RegexPattern(Rc::new(Pattern {
        source: id,
        flags: if ascii { flags } else { flags | UNICODE },
        bytes,
        engine_source,
        regex,
        assertions,
        full: OnceCell::new(),
        split_retry: OnceCell::new(),
    }))))
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
    let pattern = compiled(ctx, pattern.ok_or_else(|| missing("pattern"))?, flags)?;
    if let Some(method @ ("search" | "finditer" | "findall")) = name.strip_prefix("re.") {
        return search(ctx, method, &[pattern, string], &[]);
    }
    call(ctx, name, &[pattern, string])
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

pub(in crate::dynamic) struct Match {
    pub(in crate::dynamic) pattern: Id,
    pub(in crate::dynamic) subject: Id,
    spans: Vec<Option<Span>>,
}

impl Match {
    fn pattern<'a>(&self, ctx: &'a Context) -> &'a Pattern {
        let Value::RegexPattern(pattern) = ctx.heap.get(self.pattern) else {
            unreachable!()
        };
        pattern
    }

    /// A Scanner lexicon has exactly one participating token group in each match.
    pub(in crate::dynamic) fn lexicon_branch(&self, ctx: &Context) -> Option<usize> {
        if self.pattern(ctx).source != NONE {
            return None;
        }
        self.spans[1..]
            .iter()
            .position(Option::is_some)
            .map(|i| i + 1)
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
        let bytes = self.pattern(ctx).bytes;
        let text = subject_bytes(ctx, self.subject, bytes)?;
        // A bytearray may have changed size since the search.
        let data = &text[span.start.min(text.len())..span.end.min(text.len())];
        let value = group_value(data, bytes);
        Ok(ctx.alloc(value))
    }

    fn bounds(&self, ctx: &Context, index: usize) -> Result<(i64, i64)> {
        let Some(span) = self.spans[index] else {
            return Ok((-1, -1));
        };
        if self.pattern(ctx).bytes {
            return Ok((span.start as i64, span.end as i64));
        }
        let text = ctx.str_value(self.subject)?;
        let start = text[..span.start].chars().count() as i64;
        let end = start + text[span.range()].chars().count() as i64;
        Ok((start, end))
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
            String::from_utf8(data.to_vec()).expect("text regex captures end at UTF-8 boundaries"),
        )
    }
}

fn capture(
    ctx: &mut Context,
    pattern: Id,
    regex: &Matcher,
    bytes: bool,
    subject: Id,
    input: impl FnOnce(&[u8]) -> Input<'_>,
) -> Result<Id> {
    let text = subject_bytes(ctx, subject, bytes)?;
    let mut captures = regex.create_captures();
    regex
        .captures(input(text), &mut captures)
        .map_err(|message| Error::new("RuntimeError", message))?;
    if !captures.is_match() {
        return Ok(NONE);
    }
    Ok(ctx.alloc(Value::RegexMatch(Rc::new(Match {
        pattern,
        subject,
        spans: group_spans(&captures).collect(),
    }))))
}

fn subject_bytes(ctx: &Context, id: Id, bytes: bool) -> Result<&[u8]> {
    let value = ctx.heap.get(ctx.scalar_value(id));
    if !matches!(value, Value::Str(_) | Value::SurrogateStr(_)) {
        ctx.check_buffer_exporter(id)?;
    }
    match value {
        Value::Bytes(_) | Value::ByteArray(_) | Value::MemoryView { owner: Some(_), .. } => {
            if bytes {
                return ctx.bytes_like(id);
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
        _ => ctx.str_value(id).map(str::as_bytes).map_err(|_| {
            Error::new(
                "TypeError",
                format!(
                    "expected string or bytes-like object, got '{}'",
                    ctx.type_name(id)
                ),
            )
        }),
    }
}

/// Convert the Python character bounds before searching the original UTF-8 text.
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
    let text = subject_bytes(ctx, string, pattern.bytes)?;
    let byte = |i: usize| {
        if pattern.bytes || i >= text.len() {
            i.min(text.len())
        } else if i == 0 {
            0
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
        let text = text[..end].to_vec();
        return findall(ctx, &pattern, &text, start);
    }
    if matches!(method, "scanner" | "finditer") {
        // Resizing a bytearray or releasing a view requires a persistent scanner export.
        if matches!(
            ctx.heap.get(ctx.scalar_value(string)),
            Value::ByteArray(_) | Value::MemoryView { .. }
        ) {
            return Err(Error::new(
                "NotImplementedError",
                format!(
                    "exporting a {} to a regex scanner is unsupported",
                    ctx.type_name(string)
                ),
            ));
        }
        let scanner = ctx.alloc(Value::RegexScanner {
            pattern: receiver,
            subject: string,
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
    let matcher = pattern
        .matcher(method == "fullmatch")
        .map_err(|message| pattern_error(ctx, pattern.source, pattern.flags, message))?;
    let anchored = if method == "search" {
        Anchored::No
    } else {
        Anchored::Yes
    };
    capture(ctx, receiver, matcher, pattern.bytes, string, |text| {
        Input::new(&text[..end])
            .range(start..end)
            .anchored(anchored)
    })
}

pub(super) fn call(ctx: &mut Context, name: &str, args: &[Id]) -> Result<Id> {
    let first = argument(args, 0)?;
    if name.starts_with("match.") {
        let m = matched(ctx, first)?;
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
        return Ok(match name {
            "match.start" => ctx.int(start),
            "match.end" => ctx.int(end),
            "match.span" => {
                let start = ctx.int(start);
                let end = ctx.int(end);
                ctx.alloc(Value::Tuple(vec![start, end]))
            }
            _ => unreachable!(),
        });
    }
    let Value::RegexPattern(pattern) = ctx.heap.get(first) else {
        return Err(Error::new(
            "TypeError",
            "expected compiled regular expression",
        ));
    };
    let pattern = Rc::clone(pattern);
    let regex = pattern
        .matcher(name == "re.fullmatch")
        .map_err(|message| pattern_error(ctx, pattern.source, pattern.flags, message))?;
    let subject = argument(args, 1)?;
    match name {
        "re.match" | "re.fullmatch" => capture(ctx, first, regex, pattern.bytes, subject, |text| {
            Input::new(text).anchored(Anchored::Yes)
        }),
        _ => Err(Error::new(
            "AttributeError",
            format!("no regex operation {name}"),
        )),
    }
}

/// Collect captures starting at a byte offset in the bounded subject.
fn findall(ctx: &mut Context, pattern: &Pattern, text: &[u8], start: usize) -> Result<Id> {
    let mut found = Vec::new();
    let mut captures = pattern.regex.create_captures();
    let mut previous = None;
    while let Some(span) = split::next_span(pattern, text, start, previous, &mut captures)
        .map_err(|message| pattern.match_error(ctx, message))?
    {
        previous = Some(span);
        let fields: Vec<_> = group_spans(&captures)
            .map(|span| span.map(|s| &text[s.range()]))
            .collect();
        let value = match fields.as_slice() {
            [complete] => group_value(complete.unwrap_or_default(), pattern.bytes),
            [_, one] => group_value(one.unwrap_or_default(), pattern.bytes),
            [_, groups @ ..] => {
                let values = groups
                    .iter()
                    .map(|s| ctx.alloc(group_value(s.unwrap_or_default(), pattern.bytes)))
                    .collect();
                Value::Tuple(values)
            }
            [] => unreachable!("every regex has an overall match"),
        };
        found.push(ctx.alloc(value));
    }
    Ok(ctx.alloc(Value::List(found)))
}
