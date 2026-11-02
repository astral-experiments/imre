use super::*;

/// Substitute matches in order, retaining the allocation-free match path for literal replacements.
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
            callable_sub(ctx, &pattern, receiver, string, repl, limit)
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
        return Err(Error::new(
            "NotImplementedError",
            "boxed regex replacement templates are unsupported",
        ));
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
            return Ok(repl);
        }
        append(&mut output, replacement)?;
    } else if matches!(ctx.heap.get(string), Value::Str(_) | Value::Bytes(_)) {
        return Ok(string);
    }
    append(&mut output, &text[cursor..])?;
    Ok(if pattern.bytes {
        ctx.alloc(Value::Bytes(output))
    } else {
        ctx.text(String::from_utf8(output).expect("text regex spans end on UTF-8 boundaries"))
    })
}

/// Capture each intervening slice before the callback can change the subject. Keep
/// the returned objects themselves until every callback has run, as CPython joins last.
fn callable_sub(
    ctx: &mut Context,
    pattern: &Pattern,
    receiver: Id,
    string: Id,
    repl: Id,
    limit: isize,
) -> Result<Id> {
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
        let matched = ctx.alloc(Value::RegexMatch(Rc::new(Match {
            pattern: receiver,
            subject: string,
            spans: group_spans(&caps).collect(),
        })));
        let item = ctx.call(repl, vec![matched], Vec::new())?;
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
        return Ok(string);
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
            return Ok(*item);
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
        return Ok(ctx.alloc(Value::Bytes(output)));
    }
    for (i, &item) in pieces.iter().enumerate() {
        if ctx.string_storage(item).is_none() {
            return Err(invalid(ctx, i, item));
        }
    }
    let parts = ctx.alloc(Value::List(pieces));
    let empty = ctx.text("");
    let join = ctx.lookup_attr(empty, "join")?;
    ctx.call(join, vec![parts], Vec::new())
}

fn append(output: &mut Vec<u8>, piece: &[u8]) -> Result<()> {
    output
        .try_reserve(piece.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    output.extend_from_slice(piece);
    Ok(())
}
