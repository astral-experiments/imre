use super::*;

/// Substitute literal replacement text without allocating a Python match for each occurrence.
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
        return Err(Error::new(
            "NotImplementedError",
            "callable regex replacements are unsupported",
        ));
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
        let span = match split::next_span(&pattern, text, previous, &mut caps) {
            Ok(Some(span)) => span,
            Ok(None) => break,
            Err(message) => return Err(pattern_error(ctx, pattern.source, pattern.flags, message)),
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

fn append(output: &mut Vec<u8>, piece: &[u8]) -> Result<()> {
    output
        .try_reserve(piece.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    output.extend_from_slice(piece);
    Ok(())
}
