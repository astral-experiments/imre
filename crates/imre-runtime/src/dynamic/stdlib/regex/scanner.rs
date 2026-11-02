use super::*;

/// Advance the same cursor for either an anchored match or a forward search.
/// The call after an empty match must first try nonempty alternatives at its position.
pub(in crate::dynamic::stdlib) fn scanner_next(
    ctx: &mut Context,
    scanner: Id,
    anchored: bool,
) -> Result<Id> {
    let Value::RegexScanner {
        pattern,
        subject,
        start,
        end,
        retry,
    } = *ctx.heap.get(scanner)
    else {
        return Err(Error::new(
            "TypeError",
            "expected regular expression scanner",
        ));
    };
    let Some(mut start) = start else {
        return Ok(NONE);
    };
    let Value::RegexPattern(re) = ctx.heap.get(pattern) else {
        unreachable!()
    };
    let re = Rc::clone(re);
    if start > end {
        if anchored {
            re.check_reversed_match()?;
        }
        let Value::RegexScanner { start, .. } = ctx.heap.get_mut(scanner) else {
            unreachable!()
        };
        *start = None;
        return Ok(NONE);
    }
    // The scanner holds the storage it acquired when it was created.
    let text = &subject_bytes(ctx, ctx.scalar_value(subject), re.bytes)?[..end];
    let mut caps = re.regex.create_captures();
    let retry_match = retry
        && match split::nonempty(&re, text, start, &mut caps) {
            Ok(found) => found,
            Err(message) => return Err(re.match_error(ctx, message)),
        }
        && group_spans(&caps)
            .next()
            .flatten()
            .is_some_and(|span| !span.is_empty());
    if !retry_match && (!retry || !anchored && start < end) {
        if retry {
            start += 1;
            while !re.bytes && start < end && text[start] & 0xc0 == 0x80 {
                start += 1;
            }
        }
        re.regex
            .captures(
                Input::new(text).range(start..end).anchored(if anchored {
                    Anchored::Yes
                } else {
                    Anchored::No
                }),
                &mut caps,
            )
            .map_err(|message| re.match_error(ctx, message))?;
    } else if !retry_match {
        caps.set_pattern(None);
    }
    let spans: Vec<_> = group_spans(&caps).collect();
    let matched = caps.is_match().then(|| spans[0]).flatten();
    let Value::RegexScanner { start, retry, .. } = ctx.heap.get_mut(scanner) else {
        unreachable!()
    };
    *start = matched.map(|span| span.end);
    let Some(span) = matched else { return Ok(NONE) };
    *retry = span.is_empty();
    Ok(ctx.alloc(Value::RegexMatch(Rc::new(Match {
        pattern,
        subject,
        spans,
    }))))
}

/// Compile a capture-free text lexicon as one ordered alternation. Its outer
/// capture numbers index the original actions, just as in CPython's SRE program.
pub(super) fn compile_lexicon(ctx: &mut Context, lexicon: Id, flags: Option<Id>) -> Result<Id> {
    let flags = flags.map(|id| ctx.as_integer(id)).transpose()?.unwrap_or(0);
    if flags & !(IGNORECASE | ASCII | MULTILINE | DOTALL | UNICODE | VERBOSE) != 0 {
        return Err(Error::new(
            "NotImplementedError",
            "unsupported scanner flags",
        ));
    }
    // CPython builds the synthetic Pattern from parser.State, without the
    // implicit UNICODE flag that it would add when compiling a str spelling.
    let effective = if flags & UNICODE == 0 {
        flags | ASCII
    } else {
        flags
    };
    let ascii = effective & ASCII != 0;
    if ascii && flags & UNICODE != 0 {
        return Err(Error::new(
            "ValueError",
            "ASCII and UNICODE flags are incompatible",
        ));
    }
    let mut branches = Vec::new();
    let iterator = ctx.iterator(lexicon)?;
    while let Some(entry) = ctx.next_item(iterator)? {
        let phrase = ctx.unpack_items(entry, 2, None)?[0];
        if matches!(ctx.heap.get(phrase), Value::Bytes(_)) {
            return Err(Error::new(
                "NotImplementedError",
                "bytes scanner lexicons are unsupported",
            ));
        }
        // The Python translator validates the phrase before alternation. Passing
        // raw source to the backend can change token priority (e.g. x$|z) or
        // interpret Python literals inside a set as Rust class operations.
        let parsed = imre_regex::translate_pattern(
            ctx.str_value(phrase)?,
            flags & VERBOSE != 0,
            super::alphabet(false, ascii, flags),
            flags & DOTALL != 0,
            flags & IGNORECASE == 0 || ascii,
        )
        .map_err(|message| {
            if syntax_diagnostic(ctx, phrase, flags).is_some() {
                pattern_error(ctx, phrase, flags, message)
            } else {
                Error::new(
                    "NotImplementedError",
                    format!("unsupported scanner phrase: {message}"),
                )
            }
        })?;
        if parsed.groups.len() != 1 {
            return Err(Error::new(
                "NotImplementedError",
                "scanner phrases with captures are unsupported",
            ));
        }
        let mut source = parsed.source;
        if source.ends_with(imre_regex::FINAL_NEWLINE_ANCHOR) {
            if flags & MULTILINE == 0 {
                return Err(Error::new(
                    "NotImplementedError",
                    "terminal dollar in scanner phrases is unsupported",
                ));
            }
            source.truncate(source.len() - imre_regex::FINAL_NEWLINE_ANCHOR.len());
            source.push('$');
        }
        branches.push(format!("({source})"));
    }
    if branches.is_empty() {
        return Err(Error::new("RuntimeError", "invalid SRE code"));
    }
    let source = branches.join("|");
    let regex = build_regex(&source, false, effective, &[])
        .map_err(|e| Error::new("re.PatternError", e))?;
    Ok(ctx.alloc(Value::RegexPattern(Rc::new(Pattern {
        source: NONE,
        flags,
        bytes: false,
        engine_source: source,
        assertions: Vec::new(),
        regex,
        full: OnceCell::new(),
        split_retry: OnceCell::new(),
    }))))
}
