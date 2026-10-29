use super::*;
use regex_automata::nfa::thompson::{NFA, State};
use regex_automata::util::primitives::{NonMaxUsize, StateID};
use std::collections::HashSet;

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
    let limit = match maxsplit {
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
    let text = subject_bytes(ctx, string, pattern.bytes)?;
    let length = text.len();
    let mut pieces = Vec::new();
    let mut cursor = 0;
    let mut start = 0;
    let mut retry = false;
    let mut count = 0;
    let mut caps = pattern.regex.create_captures();
    while limit == 0 || count < limit {
        let found = if retry {
            match nonempty(&pattern, text, start, &mut caps) {
                Ok(found) => found,
                Err(message) => {
                    return Err(super::pattern_error(
                        ctx,
                        pattern.source,
                        pattern.flags,
                        message,
                    ));
                }
            }
        } else {
            true
        };
        if !found {
            if start == length {
                break;
            }
            start += 1;
            while !pattern.bytes && start < length && text[start] & 0xc0 == 0x80 {
                start += 1;
            }
            retry = false;
        }
        if !retry {
            pattern
                .regex
                .captures(Input::new(text).range(start..length), &mut caps);
        }
        let Some(span) = caps.get_group(0) else { break };
        let eol = caps.group_info().to_index(PatternID::ZERO, "imre.eol");
        let end = eol
            .and_then(|i| caps.get_group(i))
            .map_or(span.end, |nl| nl.start);
        pieces.push(Some(cursor..span.start));
        pieces.extend(
            (1..eol.unwrap_or(caps.group_len())).map(|i| caps.get_group(i).map(|s| s.range())),
        );
        cursor = end;
        start = end;
        retry = end == span.start;
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
    let nfa = pattern
        .split_retry
        .get_or_init(|| {
            NFA::compiler()
                .syntax(syntax(pattern.bytes, pattern.flags))
                .configure(NFA::config().utf8(!pattern.bytes))
                .build(&pattern.engine_source)
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)?;
    enum Frame {
        Visit(StateID, usize),
        Restore(usize, Option<NonMaxUsize>),
    }
    use Frame::{Restore, Visit};
    caps.clear();
    let mut stack = vec![Visit(nfa.start_anchored(), start)];
    let mut seen = HashSet::new();
    while let Some(frame) = stack.pop() {
        let (state, at) = match frame {
            Visit(state, at) if seen.insert((state, at)) => (state, at),
            Visit(..) => continue,
            Restore(slot, value) => {
                caps.slots_mut()[slot] = value;
                continue;
            }
        };
        let next = match nfa.state(state) {
            State::ByteRange { trans } => trans.matches(text, at).then_some(trans.next),
            State::Sparse(trans) => trans.matches(text, at),
            State::Dense(trans) => trans.matches(text, at),
            State::Look { look, next } if nfa.look_matcher().matches(*look, text, at) => {
                stack.push(Visit(*next, at));
                continue;
            }
            State::Union { alternates } => {
                stack.extend(alternates.iter().rev().map(|&s| Visit(s, at)));
                continue;
            }
            State::BinaryUnion { alt1, alt2 } => {
                stack.push(Visit(*alt2, at));
                stack.push(Visit(*alt1, at));
                continue;
            }
            State::Capture { slot, next, .. } => {
                let slot = slot.as_usize();
                stack.push(Restore(slot, caps.slots()[slot]));
                caps.slots_mut()[slot] = NonMaxUsize::new(at);
                stack.push(Visit(*next, at));
                continue;
            }
            State::Match { pattern_id } if at > start => {
                caps.set_pattern(Some(*pattern_id));
                let eol = caps.group_info().to_index(*pattern_id, "imre.eol");
                if eol
                    .and_then(|i| caps.get_group(i))
                    .is_none_or(|nl| nl.start > start)
                {
                    return Ok(true);
                }
                continue;
            }
            _ => continue,
        };
        if let Some(next) = next {
            stack.push(Visit(next, at + 1));
        }
    }
    Ok(false)
}
