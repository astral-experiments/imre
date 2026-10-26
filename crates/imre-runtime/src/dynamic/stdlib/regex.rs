use std::collections::HashMap;
use std::rc::Rc;

use regex_automata::{Anchored, Input, PatternID, meta::Regex, util::captures::Captures};

use super::{Context, Error, Id, NONE, Result, Value, argument};

// Python uses str.isalnum() plus "_" for \w and includes four additional
// ASCII separators in Unicode whitespace. The nested classes also work
// when these escapes occur inside a bracketed class in the input.
fn python_escapes(pattern: &str) -> String {
    let mut result = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }
        match chars.next() {
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

fn compile(ctx: &Context, id: Id) -> Result<Rc<Regex>> {
    if let Value::RegexPattern(regex) = ctx.heap.get(id) {
        return Ok(Rc::clone(regex));
    }
    Regex::new(&python_escapes(ctx.str_value(id)?))
        .map(Rc::new)
        .map_err(|err| Error::new("re.error", err.to_string()))
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
    let regex = compile(ctx, first)?;
    if name == "re.compile" {
        return Ok(ctx.alloc(Value::RegexPattern(regex)));
    }
    let text = ctx.str_value(argument(args, 1)?)?.to_owned();
    match name {
        "re.match" | "pattern.match" => {
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
