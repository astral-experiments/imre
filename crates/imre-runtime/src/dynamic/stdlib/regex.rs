use std::cell::OnceCell;
use std::collections::HashMap;
use std::rc::Rc;

use regex_automata::{Anchored, Input, PatternID, meta::Regex, util::captures::Captures};

use super::{Context, Error, Id, NONE, Result, Value, argument};
use crate::dynamic::builtins::bind;

pub(in crate::dynamic) const ASCII: i64 = 256;
pub(in crate::dynamic) const UNICODE: i64 = 32;

pub(in crate::dynamic) struct Pattern {
    pub(in crate::dynamic) source: String,
    pub(in crate::dynamic) flags: i64,
    regex: Regex,
    full: OnceCell<std::result::Result<Regex, String>>,
}

impl Pattern {
    fn matcher(&self, full: bool) -> Result<&Regex> {
        if !full {
            return Ok(&self.regex);
        }
        // The end assertion must participate in matching: a|ab must fully match "ab".
        self.full
            .get_or_init(|| {
                let source = python_escapes(&self.source, self.flags & ASCII != 0);
                Regex::new(&format!(r"(?:{source})\z")).map_err(|err| err.to_string())
            })
            .as_ref()
            .map_err(|message| Error::new("re.error", message.clone()))
    }
}

// Python uses str.isalnum() plus "_" for \w and includes four additional
// ASCII separators in Unicode whitespace. The nested classes also work
// when these escapes occur inside a bracketed class in the input.
fn python_escapes(pattern: &str, ascii: bool) -> String {
    let mut result = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        if c != '\\' {
            if c == '[' {
                in_class = true;
            } else if c == ']' {
                in_class = false;
            }
            result.push(c);
            continue;
        }
        match chars.next() {
            Some('d') if ascii => result.push_str("[0-9]"),
            Some('D') if ascii => result.push_str("[^0-9]"),
            Some('w') if ascii => result.push_str("[A-Za-z0-9_]"),
            Some('W') if ascii => result.push_str("[^A-Za-z0-9_]"),
            Some('s') if ascii => result.push_str(r"[\x09-\x0d\x20]"),
            Some('S') if ascii => result.push_str(r"[^\x09-\x0d\x20]"),
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
    if flags & ASCII != 0 && flags & UNICODE != 0 {
        return Err(Error::new(
            "ValueError",
            "ASCII and UNICODE flags are incompatible",
        ));
    }
    if flags & !(ASCII | UNICODE) != 0 {
        return Err(Error::new(
            "NotImplementedError",
            "unsupported native regular expression flags",
        ));
    }
    let source = ctx.str_value(id)?.to_owned();
    let ascii = flags & ASCII != 0;
    let regex = Regex::new(&python_escapes(&source, ascii))
        .map_err(|err| Error::new("re.error", err.to_string()))?;
    Ok(ctx.alloc(Value::RegexPattern(Rc::new(Pattern {
        source,
        flags: if ascii { ASCII } else { UNICODE },
        regex,
        full: OnceCell::new(),
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
    let [pattern, string, flags] = bind(name, args, kwargs, ["pattern", "string", "flags"])?;
    let string = string.ok_or_else(|| missing("string"))?;
    let pattern = compiled(ctx, pattern.ok_or_else(|| missing("pattern"))?, flags)?;
    call(ctx, name, &[pattern, string])
}

fn groups<'a>(caps: &Captures, text: &'a str) -> Vec<Option<&'a str>> {
    (0..caps.group_len())
        .map(|i| caps.get_group(i).map(|span| &text[span.range()]))
        .collect()
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
            selected.push(groups[index].clone());
        }
        if selected.is_empty() {
            selected.push(groups[0].clone());
        }
        let mut values: Vec<_> = selected
            .into_iter()
            .map(|s| s.map_or(NONE, |s| ctx.text(s)))
            .collect();
        return Ok(if values.len() == 1 {
            values.pop().unwrap()
        } else {
            ctx.alloc(Value::Tuple(values))
        });
    }
    let Value::RegexPattern(pattern) = ctx.heap.get(first) else {
        return Err(Error::new(
            "TypeError",
            "expected compiled regular expression",
        ));
    };
    let pattern = Rc::clone(pattern);
    let regex = pattern.matcher(matches!(name, "pattern.fullmatch" | "re.fullmatch"))?;
    let text = ctx.str_value(argument(args, 1)?)?.to_owned();
    match name {
        "re.match" | "pattern.match" | "re.fullmatch" | "pattern.fullmatch" => {
            let mut captures = regex.create_captures();
            regex.captures(Input::new(&text).anchored(Anchored::Yes), &mut captures);
            if !captures.is_match() {
                return Ok(NONE);
            }
            let names: HashMap<_, _> = regex
                .group_info()
                .pattern_names(PatternID::ZERO)
                .enumerate()
                .filter_map(|(i, name)| name.map(|s| (s.to_owned(), i)))
                .collect();
            let groups = groups(&captures, &text)
                .into_iter()
                .map(|value| value.map(str::to_owned))
                .collect();
            Ok(ctx.alloc(Value::RegexMatch { groups, names }))
        }
        "pattern.findall" => {
            let mut found = Vec::new();
            for captures in regex.captures_iter(&text) {
                let fields = groups(&captures, &text);
                found.push(match fields.as_slice() {
                    [complete] => ctx.text(complete.unwrap_or_default()),
                    [_, one] => ctx.text(one.unwrap_or_default()),
                    [_, groups @ ..] => {
                        let values = groups
                            .iter()
                            .map(|s| ctx.text(s.unwrap_or_default()))
                            .collect();
                        ctx.alloc(Value::Tuple(values))
                    }
                    [] => unreachable!("every regex has an overall match"),
                });
            }
            Ok(ctx.alloc(Value::List(found)))
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("no regex operation {name}"),
        )),
    }
}
