//! Partially evaluate anchored capture searches at fixed pattern compilation.

use memchr::memmem::Finder;
use regex_automata::nfa::thompson::NFA;
use regex_automata::nfa::thompson::backtrack::{self, BoundedBacktracker};
use regex_automata::util::primitives::NonMaxUsize;
use regex_automata::{Input, MatchError, PatternID};
use regex_syntax::hir::{Capture, Dot, Hir, HirKind};

type Slot = Option<NonMaxUsize>;

pub(super) struct AnchoredCaptures {
    pub(super) first: Option<u8>,
    whole: Fragment,
    delimited: Option<Delimited>,
}

impl AnchoredCaptures {
    pub(super) fn new(hir: &Hir, first: Option<u8>) -> Option<Self> {
        Some(Self {
            first,
            whole: Fragment::new(hir)?,
            delimited: Delimited::new(hir),
        })
    }

    pub(super) fn search(
        &mut self,
        input: &Input<'_>,
        slots: &mut [Slot],
    ) -> Result<Option<PatternID>, MatchError> {
        if self
            .delimited
            .as_mut()
            .is_some_and(|plan| plan.matches(input, slots))
        {
            return Ok(Some(PatternID::ZERO));
        }
        self.whole.search(input, slots)
    }

    #[cfg(test)]
    pub(super) fn max_haystack_len(&self) -> usize {
        self.whole.engine.max_haystack_len()
    }
}

/// An accepting greedy final `(.*)` follows the preferred match of the prefix.
/// Find its endpoint with a byte search over the UTF-8 string.
struct Fragment {
    engine: BoundedBacktracker,
    cache: backtrack::Cache,
    tail_slot: Option<usize>,
}

impl Fragment {
    fn new(hir: &Hir) -> Option<Self> {
        let tail = match hir.kind() {
            HirKind::Concat(parts) => parts.split_last().and_then(|(last, prefix)| {
                wildcard_slot(last, true).map(|slot| (Hir::concat(prefix.to_vec()), slot))
            }),
            _ => None,
        };
        let prefix = tail.as_ref().map_or(hir, |(prefix, _)| prefix);
        let nfa = NFA::compiler().build_from_hir(prefix).ok()?;
        let engine = BoundedBacktracker::new_from_nfa(nfa).ok()?;
        let cache = engine.create_cache();
        Some(Self {
            engine,
            cache,
            tail_slot: tail.map(|(_, slot)| slot),
        })
    }

    fn search(
        &mut self,
        input: &Input<'_>,
        slots: &mut [Slot],
    ) -> Result<Option<PatternID>, MatchError> {
        let found = self
            .engine
            .try_search_slots(&mut self.cache, input, slots)?;
        if found.is_some()
            && let Some(slot) = self.tail_slot
        {
            let start = slots[1].expect("matched prefix end");
            let rest = &input.haystack()[start.get()..input.end()];
            let end =
                NonMaxUsize::new(start.get() + memchr::memchr(b'\n', rest).unwrap_or(rest.len()));
            slots[slot] = Some(start);
            slots[slot + 1] = end;
            slots[1] = end;
        }
        Ok(found)
    }
}

/// For `prefix(.*?)literal suffix`, try the prefix's preferred endpoint and
/// the first literal in the same line. If the suffix accepts there, every
/// choice is the regex engine's preferred one. Otherwise the whole expression
/// must resolve alternatives, including any earlier choices in the prefix.
struct Delimited {
    head: Fragment,
    tail: Fragment,
    literal: Finder<'static>,
    slot: usize,
}

impl Delimited {
    fn new(hir: &Hir) -> Option<Self> {
        let HirKind::Concat(parts) = hir.kind() else {
            return None;
        };
        let (index, slot, literal) = parts.windows(2).enumerate().find_map(|(i, window)| {
            let slot = wildcard_slot(&window[0], false)?;
            let HirKind::Literal(literal) = window[1].kind() else {
                return None;
            };
            Some((i, slot, literal))
        })?;
        let head = Hir::concat(parts[..index].to_vec());
        if (head.properties().explicit_captures_len() + 1) * 2 != slot {
            return None;
        }
        // The subengine writes its whole match over the wildcard slots, then
        // its captures directly into their final positions in the caller.
        let tail = Hir::concat(
            parts[index + 1..]
                .iter()
                .map(|part| relative_groups(part, (slot / 2) as u32))
                .collect::<Option<Vec<_>>>()?,
        );
        if slot / 2 + tail.properties().explicit_captures_len()
            != hir.properties().explicit_captures_len()
        {
            return None;
        }
        Some(Self {
            head: Fragment::new(&head)?,
            tail: Fragment::new(&tail)?,
            literal: Finder::new(&literal.0).into_owned(),
            slot,
        })
    }

    fn matches(&mut self, input: &Input<'_>, slots: &mut [Slot]) -> bool {
        let (head, tail) = slots.split_at_mut(self.slot);
        if !matches!(self.head.search(input, head), Ok(Some(_))) {
            return false;
        }
        let start = head[1].expect("matched prefix end");
        let rest = &input.haystack()[start.get()..input.end()];
        let Some(relative) = self.literal.find(rest) else {
            return false;
        };
        if memchr::memchr(b'\n', &rest[..relative]).is_some() {
            return false;
        }
        let at = start.get() + relative;
        let suffix = input.clone().span(at..input.end());
        if !matches!(self.tail.search(&suffix, tail), Ok(Some(_))) {
            return false;
        }
        head[1] = tail[1];
        tail[0] = Some(start);
        tail[1] = NonMaxUsize::new(at);
        true
    }
}

/// Shift explicit capture indexes while preserving the original regex HIR.
fn relative_groups(hir: &Hir, offset: u32) -> Option<Hir> {
    let children = |parts: &[Hir]| {
        parts
            .iter()
            .map(|p| relative_groups(p, offset))
            .collect::<Option<Vec<_>>>()
    };
    Some(match hir.kind() {
        HirKind::Capture(group) => Hir::capture(Capture {
            index: group.index.checked_sub(offset)?,
            name: group.name.clone(),
            sub: Box::new(relative_groups(&group.sub, offset)?),
        }),
        HirKind::Repetition(repeat) => {
            Hir::repetition(repeat.with(relative_groups(&repeat.sub, offset)?))
        }
        HirKind::Concat(parts) => Hir::concat(children(parts)?),
        HirKind::Alternation(parts) => Hir::alternation(children(parts)?),
        _ => hir.clone(),
    })
}

/// Return the start slot of a whole capture of greedy or lazy Python dot-star.
fn wildcard_slot(hir: &Hir, greedy: bool) -> Option<usize> {
    let HirKind::Capture(group) = hir.kind() else {
        return None;
    };
    let HirKind::Repetition(repeat) = group.sub.kind() else {
        return None;
    };
    (repeat.min == 0
        && repeat.max.is_none()
        && repeat.greedy == greedy
        && *repeat.sub == Hir::dot(Dot::AnyCharExceptLF))
    .then_some(group.index as usize * 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::regexes::Pattern;
    use regex_automata::Anchored;

    #[test]
    fn literal_and_tail_plans_match_all_capture_slots() {
        for (source, delimited) in [
            (r"^(a|ab)(.*?)\x3a (?P<code>A|aA)?(?P<end>.*)", true),
            (r"a(.*?)(?:\x3a|ZZ)(.*)", false),
            (r"a(?P<body>.*?)\x3a(?P<letter>A)(?P<tail>.*)", true),
            (
                r"(?P<first>a)?(?P<body>.*?)é(?:(?P<left>a)|(?P<right>A))(?P<tail>.*)",
                true,
            ),
            (r"a(.*)\x3a(.*?)\x3a(.*)", true),
            (r"a(?P<a>.*?)(?:\x3a)(a)?(.*)", true),
            (r"a(.*?)(\x3a)(.*)", false),
            (r"a(.*?)(?:\x3a)(.*?)$", true),
            (r"a(.*)", false),
        ] {
            let mut p = Pattern::new(source);
            assert_eq!(
                p.anchored.as_ref().unwrap().delimited.is_some(),
                delimited,
                "{source}"
            );
            let mut texts = vec![
                "a:1:a:Aπ\r\nextra".to_owned(),
                "!a: Aé".to_owned(),
                "aa: :x\nAA".to_owned(),
                "éaAéa\0\n".to_owned(),
                format!("a{}:A{}", "é".repeat(512), "字".repeat(1_024)),
            ];
            // Short exhaustive inputs hit absent captures, prefix backtracking,
            // a failed first delimiter, search restarts, and embedded newlines.
            let mut layer = vec![String::new()];
            for _ in 0..=3 {
                texts.extend(layer.iter().cloned());
                layer = layer
                    .into_iter()
                    .flat_map(|s| ['a', 'A', ':', ' ', 'é', '\n'].map(|c| format!("{s}{c}")))
                    .collect();
            }
            for text in texts {
                for anchored in [true, false] {
                    let input = Input::new(&text).anchored(if anchored {
                        Anchored::Yes
                    } else {
                        Anchored::No
                    });
                    let mut slots = vec![None; p.slots.len()];
                    let want = p
                        .regex
                        .search_slots_with(&mut p.cache, &input, &mut slots)
                        .is_some();
                    let got = if anchored {
                        p.match_start(&text)
                    } else {
                        p.search(&text)
                    };
                    assert_eq!(want, got, "{source} {anchored} {text:?}");
                    if want {
                        assert_eq!(p.slots, slots, "{source} {anchored} {text:?}");
                    }
                }
            }
        }
    }
}
