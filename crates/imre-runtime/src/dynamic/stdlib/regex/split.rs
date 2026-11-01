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
    let mut matches = Matches::new(&pattern);
    let mut count = 0;
    while limit == 0 || count < limit {
        let span = match matches.next(text) {
            Ok(Some(span)) => span,
            Ok(None) => break,
            Err(message) => return Err(pattern_error(ctx, pattern.source, pattern.flags, message)),
        };
        let caps = &matches.caps;
        let eol = caps.group_info().to_index(PatternID::ZERO, "imre.eol");
        pieces.push(Some(cursor..span.start));
        pieces.extend(
            (1..eol.unwrap_or(caps.group_len())).map(|i| caps.get_group(i).map(|s| s.range())),
        );
        cursor = span.end;
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

/// Scan the original subject so anchors, UTF-8 boundaries and nonempty matches
/// immediately following empty matches agree across split, findall and substitution.
pub(super) struct Matches<'a> {
    pattern: &'a Pattern,
    pub(super) caps: Captures,
    start: usize,
    retry: bool,
}

impl<'a> Matches<'a> {
    pub(super) fn new(pattern: &'a Pattern) -> Self {
        Self {
            pattern,
            caps: pattern.regex.create_captures(),
            start: 0,
            retry: false,
        }
    }

    pub(super) fn next(
        &mut self,
        text: &[u8],
    ) -> std::result::Result<Option<std::ops::Range<usize>>, String> {
        if self.retry && !nonempty(self.pattern, text, self.start, &mut self.caps)? {
            if self.start == text.len() {
                return Ok(None);
            }
            self.start += 1;
            while !self.pattern.bytes && self.start < text.len() && text[self.start] & 0xc0 == 0x80
            {
                self.start += 1;
            }
            self.retry = false;
        }
        if !self.retry {
            self.pattern.regex.captures(
                Input::new(text).range(self.start..text.len()),
                &mut self.caps,
            );
        }
        let Some(span) = self.caps.get_group(0) else {
            return Ok(None);
        };
        let eol = self.caps.group_info().to_index(PatternID::ZERO, "imre.eol");
        let end = eol
            .and_then(|i| self.caps.get_group(i))
            .map_or(span.end, |nl| nl.start);
        self.start = end;
        self.retry = end == span.start;
        Ok(Some(span.start..end))
    }
}

/// After an empty split, Python tries the same position once more. Follow the
/// engine's priority-ordered NFA, accepting only paths that consume input.
fn nonempty(
    pattern: &Pattern,
    text: &[u8],
    start: usize,
    caps: &mut Captures,
) -> std::result::Result<bool, String> {
    if start == text.len() {
        return Ok(false);
    }
    let nfa = if let Some(nfa) = pattern.regex.guarded_nfa() {
        nfa
    } else {
        pattern
            .split_retry
            .get_or_init(|| {
                NFA::compiler()
                    .syntax(syntax(pattern.bytes, pattern.flags))
                    .configure(NFA::config().utf8(!pattern.bytes))
                    .build(&pattern.engine_source)
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?
    };
    enum Frame {
        Visit(StateID, usize),
        Restore(usize, Option<NonMaxUsize>),
    }
    use Frame::{Restore, Visit};
    let groups = nfa.group_info();
    let eol_start = groups
        .to_index(PatternID::ZERO, "imre.eol")
        .and_then(|group| groups.slot(PatternID::ZERO, group));
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
                // The synthetic final newline cannot make an empty match nonempty.
                // Discard this path before it visits states a consuming path may need.
                if at == start && Some(slot) == eol_start {
                    continue;
                }
                stack.push(Restore(slot, caps.slots()[slot]));
                caps.slots_mut()[slot] = NonMaxUsize::new(at);
                stack.push(Visit(*next, at));
                continue;
            }
            State::Match { pattern_id } if at > start => {
                caps.set_pattern(Some(*pattern_id));
                return Ok(true);
            }
            _ => continue,
        };
        if let Some(next) = next {
            stack.push(Visit(next, at + 1));
        }
    }
    Ok(false)
}
