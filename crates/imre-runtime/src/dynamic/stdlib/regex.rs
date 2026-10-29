use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt::Write;
use std::rc::Rc;

use regex_automata::{Anchored, Input, PatternID, meta::Regex, util::captures::Captures};

use super::{Context, Error, Id, NONE, Result, Value, argument};
use crate::dynamic::builtins::bind;

mod casefold;
mod split;

pub(super) use split::pattern_split;
use split::split;

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
    regex: Regex,
    full: OnceCell<std::result::Result<Regex, String>>,
    split_retry: OnceCell<std::result::Result<regex_automata::nfa::thompson::NFA, String>>,
}

impl Pattern {
    pub(in crate::dynamic) fn repr(&self, ctx: &Context) -> String {
        let mut source = ctx.repr(self.source);
        if let Some((end, _)) = source.char_indices().nth(200) {
            source.truncate(end);
        }
        let flags = if self.bytes {
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

    fn matcher(&self, full: bool) -> std::result::Result<&Regex, String> {
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
                build_regex(&format!(r"(?:{source})\z"), self.bytes, self.flags)
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

fn build_regex(source: &str, bytes: bool, flags: i64) -> std::result::Result<Regex, String> {
    Regex::builder()
        .syntax(syntax(bytes, flags))
        .configure(Regex::config().utf8_empty(!bytes))
        .build(source)
        .map_err(|err| err.to_string())
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

// Python uses str.isalnum() plus "_" for \w and includes four additional
// ASCII separators in Unicode whitespace. The nested classes also work
// when these escapes occur inside a bracketed class in the input.
fn python_escapes(pattern: &str, ascii: bool, multiline: bool) -> String {
    let mut result = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        if c != '\\' {
            if c == '$' && !in_class && !multiline && chars.peek().is_none() {
                result.push_str(imre_regex::FINAL_NEWLINE_ANCHOR);
                continue;
            }
            if c == '[' {
                in_class = true;
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
            Some('b') if ascii => result.push_str(r"(?-u:\b)"),
            Some('B') if ascii && !in_class => result.push_str(r"(?-u:\B)"),
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
    result
}

/// Report offsets in the user's spelling before byte escaping, case folding or verbose translation.
fn syntax_diagnostic(ctx: &Context, source: Id, flags: i64) -> Option<(&'static str, usize)> {
    use regex_syntax::ast::{ErrorKind, parse::ParserBuilder};
    use std::borrow::Cow;

    let text: Cow<'_, str> = match ctx.heap.get(source) {
        Value::Bytes(bytes) => Cow::Owned(bytes.iter().copied().map(char::from).collect()),
        _ => Cow::Borrowed(ctx.str_value(source).ok()?),
    };
    let error = ParserBuilder::new()
        .ignore_whitespace(flags & VERBOSE != 0)
        .build()
        .parse(&text)
        .err()?;
    let message = match error.kind() {
        ErrorKind::ClassUnclosed => "unterminated character set",
        ErrorKind::GroupUnclosed => "missing ), unterminated subpattern",
        ErrorKind::GroupUnopened => "unbalanced parenthesis",
        ErrorKind::RepetitionMissing => "nothing to repeat",
        ErrorKind::EscapeUnexpectedEof => "bad escape (end of pattern)",
        _ => return None,
    };
    Some((message, text[..error.span().start.offset].chars().count()))
}

/// Construct the public exception with the original pattern and the engine's diagnostic.
fn pattern_error(ctx: &mut Context, source: Id, flags: i64, message: String) -> Error {
    let Some(&class) = ctx.classes.get("re.PatternError") else {
        return Error::new("re.PatternError", message);
    };
    let (message, position) = match syntax_diagnostic(ctx, source, flags) {
        Some((message, position)) => (ctx.text(message), ctx.int(position as i64)),
        None => (ctx.text(message), NONE),
    };
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
    let source = match ctx.heap.get(id) {
        Value::Bytes(value) => byte_pattern(value),
        _ => ctx.str_value(id)?.to_owned(),
    };
    let ascii = bytes || flags & ASCII != 0;
    let engine_source = if flags & VERBOSE != 0 {
        let mut source = imre_regex::translate_with_ascii(&source, true, ascii)
            .map_err(|message| pattern_error(ctx, id, flags, message))?
            .source;
        if flags & MULTILINE != 0 && source.ends_with(imre_regex::FINAL_NEWLINE_ANCHOR) {
            source.truncate(source.len() - imre_regex::FINAL_NEWLINE_ANCHOR.len());
            source.push('$');
        }
        source
    } else {
        python_escapes(&source, ascii, flags & MULTILINE != 0)
    };
    let engine_source = if flags & IGNORECASE != 0 && !bytes {
        casefold::expand(&engine_source, ascii)
            .map_err(|message| pattern_error(ctx, id, flags, message))?
    } else {
        engine_source
    };
    let regex = build_regex(&engine_source, bytes, flags)
        .map_err(|message| pattern_error(ctx, id, flags, message))?;
    Ok(ctx.alloc(Value::RegexPattern(Rc::new(Pattern {
        source: id,
        flags: if ascii { flags } else { flags | UNICODE },
        bytes,
        engine_source,
        regex,
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
    if name == "re.search" {
        return search(ctx, &[pattern, string], &[]);
    }
    call(ctx, name, &[pattern, string])
}

fn groups<'a>(caps: &Captures, text: &'a [u8]) -> Vec<Option<&'a [u8]>> {
    let eol = caps.group_info().to_index(PatternID::ZERO, "imre.eol");
    let mut groups: Vec<_> = (0..eol.unwrap_or(caps.group_len()))
        .map(|i| caps.get_group(i).map(|span| &text[span.range()]))
        .collect();
    if let Some(end) = eol.and_then(|i| caps.get_group(i)) {
        groups[0] = caps.get_group(0).map(|span| &text[span.start..end.start]);
    }
    groups
}

/// Return the captures after group zero, retaining the caller's default for unmatched groups.
pub(super) fn match_groups(ctx: &mut Context, matched: Id, default: Id) -> Result<Id> {
    let Value::RegexMatch { groups, .. } = ctx.heap.get(matched) else {
        return Err(Error::new("TypeError", "expected regex match"));
    };
    let values = groups[1..].iter().map(|s| s.unwrap_or(default)).collect();
    Ok(ctx.alloc(Value::Tuple(values)))
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
    regex: &Regex,
    bytes: bool,
    subject: Id,
    input: impl FnOnce(&[u8]) -> Input<'_>,
) -> Result<Id> {
    let text = subject_bytes(ctx, subject, bytes)?;
    let mut captures = regex.create_captures();
    regex.captures(input(text), &mut captures);
    if !captures.is_match() {
        return Ok(NONE);
    }
    let names: HashMap<_, _> = regex
        .group_info()
        .pattern_names(PatternID::ZERO)
        .enumerate()
        .filter_map(|(i, name)| name.map(|s| (s.to_owned(), i)))
        .collect();
    let values: Vec<_> = groups(&captures, text)
        .into_iter()
        .map(|value| value.map(|data| group_value(data, bytes)))
        .collect();
    let groups = values
        .into_iter()
        .map(|v| v.map(|v| ctx.alloc(v)))
        .collect();
    Ok(ctx.alloc(Value::RegexMatch { groups, names }))
}

fn subject_bytes(ctx: &Context, id: Id, bytes: bool) -> Result<&[u8]> {
    match ctx.heap.get(id) {
        Value::Bytes(value) | Value::ByteArray(value) if bytes => Ok(value),
        Value::Bytes(_) | Value::ByteArray(_) => Err(Error::new(
            "TypeError",
            "cannot use a string pattern on a bytes-like object",
        )),
        _ => {
            let text = ctx.str_value(id).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "expected string or bytes-like object, got '{}'",
                        ctx.type_name(id)
                    ),
                )
            })?;
            if bytes {
                return Err(Error::new(
                    "TypeError",
                    "cannot use a bytes pattern on a string-like object",
                ));
            }
            Ok(text.as_bytes())
        }
    }
}

/// Convert the Python character bounds before searching the original UTF-8 text.
pub(super) fn search(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let receiver = argument(args, 0)?;
    let [string, pos, endpos] = crate::dynamic::builtins::bind_method_arguments(
        "search",
        &args[1..],
        kwargs,
        ["string", "pos", "endpos"],
        3,
    )?;
    let string = string.ok_or_else(|| {
        Error::new(
            "TypeError",
            "search() missing required argument 'string' (pos 1)",
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
    if start > end {
        return Ok(NONE);
    }
    capture(ctx, &pattern.regex, pattern.bytes, string, |text| {
        Input::new(&text[..end]).range(start..end)
    })
}

pub(super) fn call(ctx: &mut Context, name: &str, args: &[Id]) -> Result<Id> {
    let first = argument(args, 0)?;
    if name == "match.group" {
        let Value::RegexMatch { groups, names } = ctx.heap.get(first) else {
            return Err(Error::new("TypeError", "expected regex match"));
        };
        let mut selected = Vec::new();
        for &id in &args[1..] {
            let index = match ctx.heap.get(id) {
                Value::Str(key) => names.get(key).copied(),
                Value::Int(index) => index.to_i64().and_then(|n| usize::try_from(n).ok()),
                _ => None,
            }
            .filter(|&i| i < groups.len())
            .ok_or_else(|| Error::new("IndexError", "no such group"))?;
            selected.push(groups[index].unwrap_or(NONE));
        }
        if selected.is_empty() {
            selected.push(groups[0].unwrap_or(NONE));
        }
        return Ok(if selected.len() == 1 {
            selected[0]
        } else {
            ctx.alloc(Value::Tuple(selected))
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
        .matcher(matches!(name, "pattern.fullmatch" | "re.fullmatch"))
        .map_err(|message| pattern_error(ctx, pattern.source, pattern.flags, message))?;
    let subject = argument(args, 1)?;
    match name {
        "re.match" | "pattern.match" | "re.fullmatch" | "pattern.fullmatch" => {
            capture(ctx, regex, pattern.bytes, subject, |text| {
                Input::new(text).anchored(Anchored::Yes)
            })
        }
        "pattern.findall" => {
            let text = subject_bytes(ctx, subject, pattern.bytes)?.to_vec();
            let mut found = Vec::new();
            let mut captures = regex.create_captures();
            let mut start = 0;
            let mut retry = false;
            loop {
                if retry
                    && !split::nonempty(&pattern, &text, start, &mut captures).map_err(
                        |message| pattern_error(ctx, pattern.source, pattern.flags, message),
                    )?
                {
                    if start == text.len() {
                        break;
                    }
                    start += 1;
                    while !pattern.bytes && start < text.len() && text[start] & 0xc0 == 0x80 {
                        start += 1;
                    }
                    retry = false;
                }
                if !retry {
                    regex.captures(Input::new(&text).range(start..text.len()), &mut captures);
                }
                let Some(span) = captures.get_group(0) else {
                    break;
                };
                let eol = captures.group_info().to_index(PatternID::ZERO, "imre.eol");
                let end = eol
                    .and_then(|i| captures.get_group(i))
                    .map_or(span.end, |nl| nl.start);
                start = end;
                retry = end == span.start;
                let fields = groups(&captures, &text);
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
        _ => Err(Error::new(
            "AttributeError",
            format!("no regex operation {name}"),
        )),
    }
}
