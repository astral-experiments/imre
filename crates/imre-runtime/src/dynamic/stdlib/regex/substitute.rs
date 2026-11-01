//! Compiled-pattern substitution on the same match stream as split.

use super::*;

pub(in crate::dynamic::stdlib) fn pattern_sub(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let receiver = argument(args, 0)?;
    let [repl, string, count] = crate::dynamic::builtins::bind_method_arguments(
        "sub",
        &args[1..],
        kwargs,
        ["repl", "string", "count"],
        3,
    )?;
    let repl = repl.ok_or_else(|| {
        Error::new(
            "TypeError",
            "sub() missing required argument 'repl' (pos 1)",
        )
    })?;
    let string = string.ok_or_else(|| {
        Error::new(
            "TypeError",
            "sub() missing required argument 'string' (pos 2)",
        )
    })?;
    sub(ctx, receiver, repl, string, count)
}

enum Piece {
    Text(Vec<u8>),
    Group(usize),
}

fn template(ctx: &mut Context, pattern: &Pattern, repl: Id) -> Result<Vec<Piece>> {
    let names = pattern
        .regex
        .group_info()
        .pattern_names(PatternID::ZERO)
        .take(pattern.group_count() + 1)
        .map(|name| imre_regex::Capture {
            name: name.map(str::to_owned),
            required: false,
        })
        .collect::<Vec<_>>();
    let raw = if pattern.bytes {
        std::borrow::Cow::Owned(
            ctx.bytes_like(repl)?
                .iter()
                .copied()
                .map(char::from)
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(ctx.str_value(repl)?)
    };
    let program = imre_regex::replacement(&raw, &names)
        .map_err(|message| pattern_error(ctx, repl, 0, message.to_owned()))?;
    let mut code = program.as_slice();
    let mut pieces = Vec::new();
    while !code.is_empty() {
        let (tag, rest) = code.split_at(4);
        let tag = u32::from_le_bytes(tag.try_into().unwrap());
        code = rest;
        if tag & (1 << 31) != 0 {
            pieces.push(Piece::Group((tag & !(1 << 31)) as usize));
        } else {
            let (literal, rest) = code.split_at(tag as usize);
            let bytes = if pattern.bytes {
                // Each input byte maps to the same Latin-1 scalar during parsing.
                std::str::from_utf8(literal)
                    .unwrap()
                    .chars()
                    .map(|c| c as u8)
                    .collect()
            } else {
                literal.to_vec()
            };
            pieces.push(Piece::Text(bytes));
            code = rest;
        }
    }
    Ok(pieces)
}

pub(super) fn sub(
    ctx: &mut Context,
    receiver: Id,
    repl: Id,
    string: Id,
    count: Option<Id>,
) -> Result<Id> {
    let limit = match count {
        Some(id) => ctx
            .index_integer(id)?
            .to_i64()
            .and_then(|v| isize::try_from(v).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?,
        None => 0,
    };
    let Value::RegexPattern(pattern) = ctx.heap.get(receiver) else {
        return Err(Error::new(
            "TypeError",
            "expected compiled regular expression",
        ));
    };
    let pattern = Rc::clone(pattern);
    let template = if ctx.callable(repl) {
        None
    } else {
        Some(template(ctx, &pattern, repl)?)
    };
    ctx.with_buffer_export(string, |ctx| {
        let mut output = Vec::new();
        let mut append = |bytes: &[u8]| -> Result<()> {
            output
                .try_reserve(bytes.len())
                .map_err(|_| crate::Error::allocation())?;
            output.extend_from_slice(bytes);
            Ok(())
        };
        let mut cursor = 0;
        let mut count = 0;
        let mut matches = split::Matches::new(&pattern);
        while limit == 0 || count < limit {
            let text = subject_bytes(ctx, string, pattern.bytes)?;
            let span = match matches.next(text) {
                Ok(Some(span)) => span,
                Ok(None) => break,
                Err(message) => {
                    return Err(pattern_error(ctx, pattern.source, pattern.flags, message));
                }
            };
            append(&text[cursor..span.start])?;
            if let Some(template) = &template {
                for piece in template {
                    match piece {
                        Piece::Text(bytes) => append(bytes)?,
                        Piece::Group(0) => append(&text[span.clone()])?,
                        Piece::Group(index) => {
                            if let Some(group) = matches.caps.get_group(*index) {
                                append(&text[group.range()])?;
                            }
                        }
                    }
                }
            } else {
                let found = super::match_from_captures(ctx, pattern.bytes, string, &matches.caps)?;
                let value = ctx.call(repl, vec![found], Vec::new())?;
                if value != NONE {
                    if pattern.bytes {
                        append(ctx.bytes_like(value)?)?;
                    } else {
                        append(ctx.str_value(value)?.as_bytes())?;
                    }
                }
            }
            cursor = span.end;
            count += 1;
        }
        let tail = subject_bytes(ctx, string, pattern.bytes)?;
        if count == 0 && matches!(ctx.heap.get(string), Value::Str(_) | Value::Bytes(_)) {
            return Ok(string);
        }
        append(&tail[cursor..])?;
        Ok(ctx.alloc(if pattern.bytes {
            Value::Bytes(output)
        } else {
            Value::Str(String::from_utf8(output).expect("complete UTF-8 runs and replacement text"))
        }))
    })
}
