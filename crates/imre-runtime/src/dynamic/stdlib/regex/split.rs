use super::progress::AssertionSlot;
use super::*;
use regex_automata::nfa::thompson::{NFA, State};
use regex_automata::util::primitives::NonMaxUsize;

pub(in crate::dynamic::stdlib) fn pattern_split(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let receiver = argument(args, 0)?;
    let [string, maxsplit] = crate::dynamic::builtins::bind_method_arguments(
        "split",
        &args[1..],
        kwargs,
        ["string", "maxsplit"],
        2,
    )?;
    let string = string.ok_or_else(|| {
        Error::new(
            "TypeError",
            "split() missing required argument 'string' (pos 1)",
        )
    })?;
    split(ctx, receiver, string, maxsplit)
}

pub(super) fn split(
    ctx: &mut Context,
    receiver: Id,
    string: Id,
    maxsplit: Option<Id>,
) -> Result<Id> {
    let limit = match_limit(ctx, maxsplit)?;
    let Value::RegexPattern(pattern) = ctx.heap.get(receiver) else {
        return Err(Error::new(
            "TypeError",
            "expected compiled regular expression",
        ));
    };
    let pattern = Rc::clone(pattern);
    let text = subject_bytes(ctx, string, pattern.bytes)?;
    let length = text.len();
    let mut pieces = Vec::new();
    let mut cursor = 0;
    let mut previous = None;
    let mut count = 0;
    let mut caps = pattern.regex.create_captures();
    while limit == 0 || count < limit {
        let span = match next_span(&pattern, text, 0, previous, &mut caps) {
            Ok(Some(span)) => span,
            Ok(None) => break,
            Err(message) => return Err(pattern.match_error(ctx, message)),
        };
        let eol = caps.group_info().to_index(PatternID::ZERO, "imre.eol");
        pieces.push(Some(cursor..span.start));
        pieces.extend(
            (1..eol.unwrap_or(caps.group_len())).map(|i| caps.get_group(i).map(|s| s.range())),
        );
        cursor = span.end;
        previous = Some(span);
        count += 1;
    }
    pieces.push(Some(cursor..length));
    let mut items = Vec::with_capacity(pieces.len());
    for piece in pieces {
        items.push(match piece {
            None => NONE,
            Some(span)
                if span == (0..length)
                    && matches!(ctx.heap.get(string), Value::Str(_) | Value::Bytes(_)) =>
            {
                string
            }
            Some(span) => {
                let data = &subject_bytes(ctx, string, pattern.bytes)?[span];
                ctx.alloc(group_value(data, pattern.bytes))
            }
        });
    }
    Ok(ctx.alloc(Value::List(items)))
}

pub(super) fn match_limit(ctx: &mut Context, value: Option<Id>) -> Result<isize> {
    Ok(match value {
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
    })
}

/// Advance past the previous Python span, allowing a consuming match at an empty match's position.
pub(super) fn next_span(
    pattern: &Pattern,
    text: &[u8],
    initial: usize,
    previous: Option<Span>,
    caps: &mut Captures,
) -> std::result::Result<Option<Span>, String> {
    let mut start = previous.map_or(initial, |span| span.end);
    if start > text.len() {
        return Ok(None);
    }
    if previous.is_some_and(|span| span.is_empty()) {
        if nonempty(pattern, text, start, caps)? {
            return Ok(group_spans(caps).next().flatten());
        }
        if start == text.len() {
            return Ok(None);
        }
        start += 1;
        while !pattern.bytes && start < text.len() && text[start] & 0xc0 == 0x80 {
            start += 1;
        }
    }
    pattern
        .regex
        .captures(Input::new(text).range(start..text.len()), caps)?;
    Ok(group_spans(caps).next().flatten())
}

/// After an empty split, Python tries the same position once more. Follow the
/// engine's priority-ordered NFA, accepting only paths that consume input.
pub(super) fn nonempty(
    pattern: &Pattern,
    text: &[u8],
    start: usize,
    caps: &mut Captures,
) -> std::result::Result<bool, String> {
    if start == text.len() {
        return Ok(false);
    }
    let input = Input::new(text)
        .range(start..text.len())
        .anchored(Anchored::Yes);
    if let Some(found) = pattern.regex.asserted_search(input.clone(), caps, true)? {
        return Ok(found);
    }
    Ok(search_nfa(retry_nfa(pattern)?, &[], input, caps, true))
}

fn retry_nfa(pattern: &Pattern) -> std::result::Result<&NFA, String> {
    if let Some(nfa) = pattern.regex.guarded_nfa() {
        return Ok(nfa);
    }
    pattern
        .split_retry
        .get_or_init(|| {
            NFA::compiler()
                .syntax(syntax(pattern.bytes, pattern.engine_flags()))
                .configure(NFA::config().utf8(!pattern.bytes))
                .build(&pattern.engine_source)
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Without a repeated capture, participating groups close in tree postorder.
pub(super) fn single_visit_order(pattern: &Pattern) -> Option<Vec<usize>> {
    use regex_syntax::hir::{Hir, HirKind};
    fn visit(hir: &Hir, public: usize, order: &mut Vec<usize>) -> Option<()> {
        if let HirKind::Repetition(rep) = hir.kind()
            && rep.max.is_none_or(|max| max > 1)
            && rep.sub.properties().explicit_captures_len() != 0
        {
            return None;
        }
        for child in hir.kind().subs() {
            visit(child, public, order)?;
        }
        if let HirKind::Capture(group) = hir.kind()
            && group.index as usize <= public
        {
            order.push(group.index as usize);
        }
        Some(())
    }
    let hir = regex_automata::util::syntax::parse_with(
        &pattern.engine_source,
        &syntax(pattern.bytes, pattern.engine_flags()),
    )
    .ok()?;
    let mut order = Vec::new();
    visit(&hir, pattern.group_count(), &mut order)?;
    Some(order)
}

/// Recover the last public close on the priority-ordered path to the selected
/// endpoint. Keep the entire search haystack so anchors and guards see endpos.
pub(super) fn last_close(pattern: &Pattern, text: &[u8], caps: &Captures) -> Option<usize> {
    let span = caps.get_group(0)?;
    let input = Input::new(text)
        .range(span.start..text.len())
        .anchored(Anchored::Yes);
    let (nfa, assertions) = if let Some(parts) = pattern.regex.asserted_nfa() {
        parts
    } else {
        (retry_nfa(pattern).ok()?, &[][..])
    };
    let mut traced = pattern.regex.create_captures();
    let last = walk_nfa(nfa, assertions, input, &mut traced, false, Some(span.end))?;
    // Full matches can omit the synthetic final-newline group.
    (last != 0 && group_spans(caps).eq(group_spans(&traced))).then_some(last)
}

/// Explore each byte's NFA frontier in priority order. Capture-free guards depend
/// only on the offset, so the first path reaching a state supplies its captures.
pub(super) fn search_nfa(
    nfa: &NFA,
    assertions: &[AssertionSlot],
    input: Input<'_>,
    caps: &mut Captures,
    nonempty: bool,
) -> bool {
    walk_nfa(nfa, assertions, input, caps, nonempty, None).is_some()
}

// Zero denotes a winning match without a public capture. The last group is
// part of each frontier entry, so a losing alternative cannot publish it.
fn walk_nfa(
    nfa: &NFA,
    assertions: &[AssertionSlot],
    input: Input<'_>,
    caps: &mut Captures,
    nonempty: bool,
    required_end: Option<usize>,
) -> Option<usize> {
    caps.clear();
    if input.is_done() {
        return None;
    }
    let start = input.start();
    let text = input.haystack();
    let entry = match input.get_anchored() {
        Anchored::Yes => nfa.start_anchored(),
        Anchored::No => nfa.start_unanchored(),
        Anchored::Pattern(pid) => match nfa.start_pattern(pid) {
            Some(state) => state,
            None => return None,
        },
    };
    let groups = caps.group_info();
    let eol_start = groups
        .to_index(PatternID::ZERO, "imre.eol")
        .and_then(|group| groups.slot(PatternID::ZERO, group));
    let mut stack = vec![(entry, Rc::new(vec![None; caps.slots().len()]), 0)];
    let mut following = Vec::new();
    let mut seen_at = vec![usize::MAX; nfa.states().len()];
    let mut best = None;
    for at in start..=required_end.unwrap_or(input.end()) {
        while let Some((state, mut slots, mut last)) = stack.pop() {
            if seen_at[state.as_usize()] == at {
                continue;
            }
            seen_at[state.as_usize()] = at;
            let consume = match nfa.state(state) {
                State::ByteRange { trans } => trans.matches(text, at).then_some(trans.next),
                State::Sparse(trans) => trans.matches(text, at),
                State::Dense(trans) => trans.matches(text, at),
                State::Look { look, next } if nfa.look_matcher().matches(*look, text, at) => {
                    stack.push((*next, slots, last));
                    continue;
                }
                State::Union { alternates } => {
                    stack.extend(
                        alternates
                            .iter()
                            .rev()
                            .map(|&s| (s, Rc::clone(&slots), last)),
                    );
                    continue;
                }
                State::BinaryUnion { alt1, alt2 } => {
                    stack.push((*alt2, Rc::clone(&slots), last));
                    stack.push((*alt1, slots, last));
                    continue;
                }
                State::Capture { slot, next, .. } => {
                    let slot = match assertions.get(slot.as_usize()) {
                        Some(AssertionSlot::Guard(guard)) => {
                            if guard.matches(&input, at) {
                                stack.push((*next, slots, last));
                            }
                            continue;
                        }
                        Some(AssertionSlot::Ignore) => {
                            stack.push((*next, slots, last));
                            continue;
                        }
                        Some(AssertionSlot::Public(slot)) => *slot,
                        None => slot.as_usize(),
                    };
                    // The unanchored prefix may visit the middle of a UTF-8 code point.
                    if slot == 0 && nfa.is_utf8() && !input.is_char_boundary(at) {
                        continue;
                    }
                    // A synthetic final newline cannot make the nonempty retry succeed.
                    if nonempty && at == start && Some(slot) == eol_start {
                        continue;
                    }
                    if slot > 1 && slot % 2 == 1 && eol_start.is_none_or(|eol| slot < eol) {
                        last = slot / 2;
                    }
                    Rc::make_mut(&mut slots)[slot] = NonMaxUsize::new(at);
                    stack.push((*next, slots, last));
                    continue;
                }
                State::Match { pattern_id }
                    if (!nonempty || at > start) && required_end.is_none_or(|end| end == at) =>
                {
                    // Only threads already in the next frontier outrank this match.
                    best = Some((*pattern_id, slots, last));
                    break;
                }
                _ => continue,
            };
            if let Some(next) = consume
                && at < input.end()
            {
                following.push((next, slots, last));
            }
        }
        if following.is_empty() {
            break;
        }
        stack.clear();
        std::mem::swap(&mut stack, &mut following);
        stack.reverse();
    }
    if let Some((id, slots, last)) = best {
        caps.slots_mut().copy_from_slice(&slots);
        caps.set_pattern(Some(id));
        return Some(last);
    }
    None
}
