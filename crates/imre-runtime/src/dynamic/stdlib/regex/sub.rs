use super::*;
use imre_regex::{ReplacementError, ReplacementPart};

/// Substitute matches in order, retaining the allocation-free match path for literal replacements.
pub(in crate::dynamic::stdlib) fn pattern_sub(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (value, substitutions) = substitute(ctx, method, args, kwargs)?;
    if method == "subn" {
        let count = ctx.int(substitutions as i64);
        Ok(ctx.alloc(Value::Tuple(vec![value, count])))
    } else {
        Ok(value)
    }
}

/// Both entry points count completed substitutions in the same matcher loop.
fn substitute(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<(Id, isize)> {
    let receiver = argument(args, 0)?;
    let [Some(repl), Some(string), count] = crate::dynamic::builtins::bind_builtin_arguments(
        method,
        &args[1..],
        kwargs,
        ["repl", "string", "count"],
        2,
    )?
    else {
        unreachable!("the replacement and input were bound above")
    };
    let limit = split::match_limit(ctx, count)?;
    let Value::RegexPattern(pattern) = ctx.heap.get(receiver) else {
        return Err(Error::new(
            "TypeError",
            "expected compiled regular expression",
        ));
    };
    let pattern = Rc::clone(pattern);
    if ctx.callable(repl) {
        return ctx.with_buffer_export(string, |ctx| {
            expanded_sub(ctx, &pattern, receiver, string, limit, |ctx, m| {
                let matched = ctx.alloc(Value::RegexMatch(Rc::new(m)));
                ctx.call(repl, vec![matched], Vec::new())
            })
        });
    }
    let (replacement, replacement_is_bytes) = match ctx.heap.get(ctx.scalar_value(repl)) {
        Value::Str(s) => (s.as_bytes(), false),
        Value::Bytes(_) | Value::ByteArray(_) | Value::MemoryView { .. } => {
            (ctx.bytes_like(repl)?, true)
        }
        Value::SurrogateStr(_) => {
            return Err(Error::new(
                "NotImplementedError",
                "regex replacement strings containing surrogates are unsupported",
            ));
        }
        _ => {
            // CPython attempts to cache a nonliteral template before parsing it.
            ctx.dictionary_hash(repl)?;
            return Err(Error::new(
                "TypeError",
                format!(
                    "decoding to str: need a bytes-like object, {} found",
                    ctx.type_name(repl)
                ),
            ));
        }
    };
    if replacement.contains(&b'\\') {
        return template_sub(
            ctx,
            &pattern,
            receiver,
            repl,
            string,
            limit,
            replacement_is_bytes,
        );
    }
    let text = subject_bytes(ctx, string, pattern.bytes)?;
    let mut output = Vec::new();
    let mut caps = pattern.regex.create_captures();
    let mut previous = None;
    let mut cursor = 0;
    let mut substitutions = 0;
    while limit == 0 || substitutions < limit {
        let span = match split::next_span(&pattern, text, 0, previous, &mut caps) {
            Ok(Some(span)) => span,
            Ok(None) => break,
            Err(message) => return Err(pattern.match_error(ctx, message)),
        };
        if pattern.bytes != replacement_is_bytes {
            let expected = if pattern.bytes {
                "a bytes-like object"
            } else {
                "str instance"
            };
            let index = usize::from(span.start > 0);
            return Err(Error::new(
                "TypeError",
                format!(
                    "sequence item {index}: expected {expected}, {} found",
                    ctx.type_name(repl)
                ),
            ));
        }
        if previous.is_some() {
            append(&mut output, replacement)?;
        }
        append(&mut output, &text[cursor..span.start])?;
        previous = Some(span);
        cursor = span.end;
        substitutions += 1;
    }
    if let Some(span) = previous {
        if substitutions == 1
            && span.range() == (0..text.len())
            && matches!(ctx.heap.get(repl), Value::Str(_) | Value::Bytes(_))
        {
            return Ok((repl, substitutions));
        }
        append(&mut output, replacement)?;
    } else if matches!(ctx.heap.get(string), Value::Str(_) | Value::Bytes(_)) {
        return Ok((string, 0));
    }
    append(&mut output, &text[cursor..])?;
    let value = if pattern.bytes {
        ctx.alloc(Value::Bytes(output))
    } else {
        ctx.text(String::from_utf8(output).expect("text regex spans end on UTF-8 boundaries"))
    };
    Ok((value, substitutions))
}

/// Parse the template before searching; unmatched captures contribute no text to the expansion.
fn template_sub(
    ctx: &mut Context,
    pattern: &Pattern,
    receiver: Id,
    repl: Id,
    string: Id,
    limit: isize,
    replacement_is_bytes: bool,
) -> Result<(Id, isize)> {
    // CPython's template cache retries unhashable buffers, but propagates invalid memoryview hashes.
    if matches!(ctx.heap.get(repl), Value::MemoryView { .. })
        && let Err(error) = ctx.dictionary_hash(repl)
    {
        let ty = ctx.builtin("TypeError");
        if !ctx.raised_error_matches(&error, ty) {
            return Err(error);
        }
    }
    if ctx.scalar_value(repl) != repl {
        return Err(Error::new(
            "NotImplementedError",
            "boxed regex replacement templates with custom scalar subclasses are unsupported",
        ));
    }
    let source = if replacement_is_bytes {
        ctx.bytes_like(repl)?
            .iter()
            .map(|&b| char::from(b))
            .collect()
    } else {
        ctx.str_value(ctx.scalar_value(repl))?.to_owned()
    };
    let info = pattern.regex.group_info();
    let limit_digits = crate::integer_text::decimal_digit_limit();
    let parts =
        imre_regex::parse_replacement(&source, pattern.group_count() + 1, limit_digits, |name| {
            group_names::valid(name, replacement_is_bytes)
                .then(|| info.to_index(PatternID::ZERO, name))
                .flatten()
        });
    let parts = match parts {
        Ok(parts) => parts,
        Err(err) => {
            let (message, position) = match err {
                ReplacementError::DigitLimit { count } => {
                    return Err(Error::new(
                        "ValueError",
                        format!(
                            "Exceeds the limit ({limit_digits} digits) for integer string conversion: value has {count} digits; use sys.set_int_max_str_digits() to increase the limit"
                        ),
                    ));
                }
                ReplacementError::Syntax {
                    message, position, ..
                } => (message, position),
                ReplacementError::GroupName { name, position } => {
                    if group_names::valid(&name, replacement_is_bytes) {
                        return Err(Error::new(
                            "IndexError",
                            format!(
                                "unknown group name {}",
                                crate::objects::exceptions::string_repr(&name)
                            ),
                        ));
                    }
                    (group_names::bad_name(&name, replacement_is_bytes), position)
                }
            };
            let source = if replacement_is_bytes && !matches!(ctx.heap.get(repl), Value::Bytes(_)) {
                ctx.alloc(Value::Bytes(ctx.bytes_like(repl)?.to_vec()))
            } else {
                repl
            };
            return Err(pattern_error_at(ctx, source, message, Some(position)));
        }
    };
    let parts: Vec<ReplacementPart<Id>> = parts
        .into_iter()
        .map(|part| match part {
            ReplacementPart::Group(index) => ReplacementPart::Group(index),
            ReplacementPart::Literal(text) => {
                let value = if replacement_is_bytes {
                    ctx.alloc(Value::Bytes(text.chars().map(|ch| ch as u8).collect()))
                } else {
                    ctx.text(text)
                };
                ReplacementPart::Literal(value)
            }
        })
        .collect();
    let empty = if replacement_is_bytes {
        ctx.alloc(Value::Bytes(Vec::new()))
    } else {
        ctx.text("")
    };
    let join = ctx.lookup_attr(empty, "join")?;
    let expand = |ctx: &mut Context, m: Match| {
        let mut pieces = Vec::new();
        for part in &parts {
            match *part {
                ReplacementPart::Literal(id) => pieces.push(id),
                ReplacementPart::Group(index) => {
                    let item = m.group(ctx, index, NONE)?;
                    if item != NONE {
                        pieces.push(item);
                    }
                }
            }
        }
        let pieces = ctx.alloc(Value::List(pieces));
        ctx.call(join, vec![pieces], Vec::new())
    };
    ctx.with_buffer_export(string, |ctx| {
        expanded_sub(ctx, pattern, receiver, string, limit, expand)
    })
}

/// Capture each intervening slice before the callback can change the subject. Keep
/// the returned objects themselves until every callback has run, as CPython joins last.
fn expanded_sub(
    ctx: &mut Context,
    pattern: &Pattern,
    receiver: Id,
    string: Id,
    limit: isize,
    mut expand: impl FnMut(&mut Context, Match) -> Result<Id>,
) -> Result<(Id, isize)> {
    let length = subject_bytes(ctx, string, pattern.bytes)?.len();
    let mut pieces = Vec::new();
    let mut add = |item| -> Result<()> {
        pieces
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        pieces.push(item);
        Ok(())
    };
    let mut caps = pattern.regex.create_captures();
    let mut previous = None;
    let mut cursor = 0;
    let mut count = 0;
    while limit == 0 || count < limit {
        let text = subject_bytes(ctx, string, pattern.bytes)?;
        let span = match split::next_span(pattern, text, 0, previous, &mut caps) {
            Ok(Some(span)) => span,
            Ok(None) => break,
            Err(message) => return Err(pattern.match_error(ctx, message)),
        };
        if cursor < span.start {
            let gap = group_value(&text[cursor..span.start], pattern.bytes);
            add(ctx.alloc(gap))?;
        }
        let mark = ctx.heap.temporary_mark();
        let matched = Match {
            pattern: receiver,
            subject: string,
            spans: group_spans(&caps).collect(),
        };
        let item = expand(ctx, matched)?;
        ctx.heap.release_temporaries_since(mark);
        if item != NONE {
            ctx.heap.keep(item);
            add(item)?;
        }
        previous = Some(span);
        cursor = span.end;
        count += 1;
    }
    if previous.is_none() && matches!(ctx.heap.get(string), Value::Str(_) | Value::Bytes(_)) {
        return Ok((string, 0));
    }
    if cursor < length {
        let tail = group_value(
            &subject_bytes(ctx, string, pattern.bytes)?[cursor..],
            pattern.bytes,
        );
        add(ctx.alloc(tail))?;
    }

    let invalid = |ctx: &Context, i: usize, item: Id| {
        let expected = if pattern.bytes {
            "a bytes-like object"
        } else {
            "str instance"
        };
        Error::new(
            "TypeError",
            format!(
                "sequence item {i}: expected {expected}, {} found",
                ctx.type_name(item)
            ),
        )
    };
    if pattern.bytes {
        if let [item] = pieces.as_slice()
            && matches!(ctx.heap.get(*item), Value::Bytes(_))
        {
            return Ok((*item, count));
        }
        let mut output = Vec::new();
        for (i, item) in pieces.into_iter().enumerate() {
            ctx.check_buffer_exporter(item)?;
            let data = match ctx.heap.get(ctx.scalar_value(item)) {
                Value::Array(array) => array.as_bytes(),
                _ => ctx.bytes_like(item).map_err(|_| invalid(ctx, i, item))?,
            };
            append(&mut output, data)?;
        }
        return Ok((ctx.alloc(Value::Bytes(output)), count));
    }
    for (i, &item) in pieces.iter().enumerate() {
        if ctx.string_storage(item).is_none() {
            return Err(invalid(ctx, i, item));
        }
    }
    let parts = ctx.alloc(Value::List(pieces));
    let empty = ctx.text("");
    let join = ctx.lookup_attr(empty, "join")?;
    let value = ctx.call(join, vec![parts], Vec::new())?;
    Ok((value, count))
}

fn append(output: &mut Vec<u8>, piece: &[u8]) -> Result<()> {
    output
        .try_reserve(piece.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    output.extend_from_slice(piece);
    Ok(())
}
