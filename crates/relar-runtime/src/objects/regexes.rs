//! Fixed string patterns and immutable snapshots of participating capture spans.

use std::cell::Cell;
use std::ptr;
use std::rc::Rc;

use memchr::memmem::Finder;
use regex_automata::meta::{self, Regex};
use regex_automata::nfa::thompson::backtrack::{self, BoundedBacktracker};
use regex_automata::util::primitives::NonMaxUsize;
use regex_automata::{Anchored, Input, PatternID};
use regex_syntax::hir::{Hir, HirKind, literal::Extractor};

use super::{
    Error, Int, Kind, Object, Storage, StorageCell, Str, acquire_rc, fail, finish, object,
};

pub(super) struct Pattern {
    regex: Regex,
    cache: meta::Cache,
    slots: Vec<Option<NonMaxUsize>>,
    required: Option<Finder<'static>>,
    anchored: Option<PrefixSearch>,
}

struct PrefixSearch {
    first: u8,
    engine: BoundedBacktracker,
    cache: backtrack::Cache,
}

/// Every match must contain this literal; branches and optional repetitions do
/// not supply one. In a concatenation any required literal is sufficient.
fn required_literal(hir: &Hir) -> Option<&[u8]> {
    match hir.kind() {
        HirKind::Literal(literal) => Some(&literal.0),
        HirKind::Capture(capture) => required_literal(&capture.sub),
        HirKind::Repetition(repeat) if repeat.min > 0 => required_literal(&repeat.sub),
        HirKind::Concat(parts) => parts
            .iter()
            .filter_map(required_literal)
            .max_by_key(|literal| literal.len()),
        _ => None,
    }
}

impl Pattern {
    fn new(source: &str) -> Self {
        let hir = regex_syntax::Parser::new()
            .parse(source)
            .expect("validated fixed regex");
        let regex = Regex::builder()
            .build_from_hir(&hir)
            .expect("validated fixed regex");
        let cache = regex.create_cache();
        let slots = vec![None; regex.group_info().slot_len()];
        let prefixes = Extractor::new().extract(&hir);
        let first = prefixes
            .longest_common_prefix()
            .and_then(|s| s.first().copied())
            .filter(u8::is_ascii);
        let anchored = first.and_then(|first| {
            let engine = BoundedBacktracker::new(source).ok()?;
            let cache = engine.create_cache();
            Some(PrefixSearch {
                first,
                engine,
                cache,
            })
        });
        let required = required_literal(&hir)
            .filter(|s| !s.is_empty() && !(s.len() == 1 && Some(s[0]) == first))
            .map(|s| Finder::new(s).into_owned());
        Self {
            regex,
            cache,
            slots,
            anchored,
            required,
        }
    }

    fn search(&mut self, text: &str) -> bool {
        let bytes = text.as_bytes();
        if self
            .required
            .as_ref()
            .is_some_and(|literal| literal.find(bytes).is_none())
        {
            return false;
        }
        let mut start = 0;
        if let Some(prefix) = &mut self.anchored {
            let Some(at) = memchr::memchr(prefix.first, bytes) else {
                return false;
            };
            let input = Input::new(text)
                .span(at..bytes.len())
                .anchored(Anchored::Yes);
            // One anchored attempt avoids the general engine's forward/reverse
            // search on hits. A size-limit error or further candidates fall back
            // to one unanchored search, which preserves the worst-case time bound.
            match prefix
                .engine
                .try_search_slots(&mut prefix.cache, &input, &mut self.slots)
            {
                Ok(Some(_)) => return true,
                Ok(None) => {
                    let Some(next) = memchr::memchr(prefix.first, &bytes[at + 1..]) else {
                        return false;
                    };
                    start = at + 1 + next;
                }
                Err(_) => start = at,
            }
        }
        self.regex
            .search_slots_with(
                &mut self.cache,
                &Input::new(text).span(start..bytes.len()),
                &mut self.slots,
            )
            .is_some()
    }
}

pub(super) struct Match {
    pattern: Rc<Object>,
    text: Rc<Str>,
    spans: Spans,
}

// Directly storing short capture snapshots must fit an existing Object payload.
const _: () = assert!(size_of::<Match>() <= size_of::<super::mappings::MappingIterator>());

enum Spans {
    Short {
        len: u8,
        slots: [u16; 10],
        ascii_prefix: Cell<u16>,
    },
    Long(Box<[Option<NonMaxUsize>]>),
}

impl Spans {
    fn new(slots: &[Option<NonMaxUsize>], text_len: usize) -> Self {
        if slots.len() <= 10 && text_len < usize::from(u16::MAX) {
            let mut short = [u16::MAX; 10];
            for (dst, slot) in short.iter_mut().zip(slots) {
                *dst = slot.map_or(u16::MAX, |s| s.get() as u16);
            }
            Self::Short {
                len: slots.len() as u8,
                slots: short,
                ascii_prefix: Cell::new(0),
            }
        } else {
            Self::Long(slots.into())
        }
    }

    /// Outer None is an unknown index. Inner None is a known unmatched group.
    fn get(&self, index: usize) -> Option<Option<(usize, usize)>> {
        let at = index.checked_mul(2)?;
        match self {
            Self::Short { len, slots, .. } => {
                if at >= usize::from(*len) {
                    return None;
                }
                Some(
                    (slots[at] != u16::MAX)
                        .then(|| (usize::from(slots[at]), usize::from(slots[at + 1]))),
                )
            }
            Self::Long(slots) => Some(
                slots
                    .get(at)?
                    .zip(slots[at + 1])
                    .map(|(a, b)| (a.get(), b.get())),
            ),
        }
    }

    /// Successive bounds can count the ASCII portion of a short input once.
    fn char_offset(&self, text: &str, byte: usize) -> usize {
        if let Self::Short { ascii_prefix, .. } = self {
            let seen = ascii_prefix.get();
            if seen != u16::MAX {
                if byte <= usize::from(seen) {
                    return byte;
                }
                if text.as_bytes()[usize::from(seen)..byte].is_ascii() {
                    ascii_prefix.set(byte as u16);
                    return byte;
                }
                ascii_prefix.set(u16::MAX);
            }
        }
        text[..byte].chars().count()
    }
}

fn handle(storage: Storage) -> *const Object {
    Rc::into_raw(Rc::new(Object(StorageCell::new(storage))))
}

/// Instantiates a literal already validated by the compiler with this regex engine.
///
/// # Safety
/// `data` holds the `len` UTF-8 bytes of the translated pattern, or is empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_regex_compile(data: *const u8, len: usize) -> *const Object {
    let source = if len == 0 {
        ""
    } else {
        // SAFETY: the compiler emits initialized static UTF-8 bytes of this exact length.
        unsafe { str::from_utf8_unchecked(std::slice::from_raw_parts(data, len)) }
    };
    handle(Storage::RegexPattern(Box::new(Pattern::new(source))))
}

/// Returns a rooted snapshot of the leftmost match, or a null optional on a miss.
///
/// # Safety
/// Both arguments are rooted handles of the declared types. The search cannot reenter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_regex_search(
    pattern: *const Object,
    text: *const Str,
) -> *const Object {
    // SAFETY: both inputs outlive the search. Mutating engine scratch cannot call
    // user code, and only copied capture spans escape this private storage borrow.
    unsafe {
        object(pattern).0.with_mut(|storage| {
            let Storage::RegexPattern(compiled) = storage else {
                fail("SystemError", "expected a compiled regex");
            };
            if !compiled.search(&*text) {
                return ptr::null();
            }
            handle(Storage::RegexMatch(Match {
                pattern: acquire_rc(pattern as usize),
                text: acquire_rc(text as usize),
                spans: Spans::new(&compiled.slots, (&*text).len()),
            }))
        })
    }
}

impl Match {
    /// Distinguishes an unknown selector (IndexError) from a nonparticipating group.
    #[inline(always)] // Keep the scalar span in the calling group or bound operation.
    unsafe fn span(&self, selector: usize, named: u32) -> Result<Option<(usize, usize)>, Error> {
        let index = if named == 0 {
            // SAFETY: the ABI roots an integer selector's canonical word. Every
            // in-range group index is a small integer, so a boxed integer misses.
            unsafe { Int::borrow(selector) }
                .as_small()
                .and_then(|i| usize::try_from(i).ok())
        } else {
            // SAFETY: named selectors root a string. The match owns the pattern,
            // distinct from its own object, and read access cannot call user code.
            unsafe {
                let name = (&*(selector as *const Str)).as_str();
                self.pattern.0.with(|storage| {
                    let Storage::RegexPattern(pattern) = storage else {
                        unreachable!()
                    };
                    let groups = pattern.regex.group_info();
                    if pattern.slots.len() <= 16 {
                        groups
                            .pattern_names(PatternID::ZERO)
                            .position(|n| n == Some(name))
                    } else {
                        groups.to_index(PatternID::ZERO, name)
                    }
                })
            }
        };
        index
            .and_then(|i| self.spans.get(i))
            .ok_or_else(|| Error::new(Kind::IndexError, "no such group"))
    }
}

impl Storage {
    fn regex_match(&self) -> &Match {
        match self {
            Self::RegexMatch(value) => value,
            _ => fail("SystemError", "expected a regex match"),
        }
    }
}

/// Returns an owned substring, including a live empty string for an empty capture.
///
/// # Safety
/// Root a Match and one canonical int (`named == 0`) or string (`named != 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_regex_group(
    value: *const Object,
    selector: usize,
    named: u32,
) -> *const Str {
    // SAFETY: the match and selector are rooted; no borrowed substring escapes.
    let result = unsafe {
        object(value).0.with(|storage| {
            let matched = storage.regex_match();
            Ok(match matched.span(selector, named)? {
                Some((start, end)) if start == 0 && end == matched.text.len() => {
                    Rc::into_raw(Rc::clone(&matched.text))
                }
                Some((start, end)) => Rc::into_raw(Rc::new(Str::new(&matched.text[start..end]))),
                None => ptr::null(),
            })
        })
    };
    finish(result, ptr::null())
}

/// Checks participation without allocating the capture substring.
///
/// # Safety
/// Root the same Match and selector types as `relar_regex_group`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_regex_present(
    value: *const Object,
    selector: usize,
    named: u32,
) -> u32 {
    // SAFETY: the match and selector are rooted throughout this read.
    let result = unsafe {
        object(value).0.with(|storage| {
            storage
                .regex_match()
                .span(selector, named)
                .map(|span| u32::from(span.is_some()))
        })
    };
    finish(result, 0)
}

/// Returns Python's code-point start or end index, or -1 for a nonparticipating group.
///
/// # Safety
/// Root the same match and selector types as `relar_regex_group`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_regex_bound(
    value: *const Object,
    selector: usize,
    named: u32,
    end: u32,
) -> usize {
    // SAFETY: the caller roots both handles. Every regex span is a UTF-8 boundary.
    let result = unsafe {
        object(value).0.with(|storage| {
            let matched = storage.regex_match();
            Ok(match matched.span(selector, named)? {
                Some((start, stop)) => Int::from_usize(
                    matched
                        .spans
                        .char_offset(&matched.text, if end == 0 { start } else { stop }),
                )
                .into_raw(),
                None => Int::small(-1).unwrap().into_raw(),
            })
        })
    };
    finish(result, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{
        relar_error_pending, relar_error_take, relar_exception_matches,
    };

    #[test]
    fn wide_and_long_capture_snapshots_preserve_exact_offsets() {
        let pattern = "(a)(b)?(c)?(d)?(e)?(?P<last>f)";
        for text in ["af".to_owned(), format!("{}af", "é".repeat(40_000))] {
            let prefix_chars = text.chars().count() - 2;
            let text = Rc::new(Str::from(text));
            // SAFETY: the helper owns each ABI handle until its last access.
            unsafe {
                let compiled = Rc::from_raw(relar_regex_compile(pattern.as_ptr(), pattern.len()));
                let found =
                    Rc::from_raw(relar_regex_search(Rc::as_ptr(&compiled), Rc::as_ptr(&text)));
                let m = Rc::as_ptr(&found);
                let last = Str::new("last");
                assert_eq!(
                    Int::borrow(relar_regex_bound(m, 1, 0, 0)).as_small(),
                    Some(prefix_chars as i64)
                );
                assert_eq!(
                    Int::borrow(relar_regex_bound(m, 13, 0, 1)).as_small(),
                    Some(prefix_chars as i64 + 2)
                );
                assert_eq!(
                    *Rc::from_raw(relar_regex_group(m, &last as *const Str as usize, 1)),
                    "f"
                );
                assert_eq!(relar_regex_present(m, 5, 0), 0);
                assert_eq!(relar_regex_present(m, 13, 0), 1);
                assert_eq!(relar_regex_present(m, 15, 0), 0);
                let error = Rc::from_raw(relar_error_take());
                assert_eq!(
                    relar_exception_matches(Rc::as_ptr(&error), Kind::IndexError as u32),
                    1
                );
            }
        }
    }

    #[test]
    fn failed_first_prefix_and_bounded_engine_limit_fall_back() {
        let mut pattern = Pattern::new("a[ab]+Z");
        assert!(pattern.search("a!abc abbaZ"));
        assert_eq!(pattern.slots[0].unwrap().get(), 6);
        let limit = pattern.anchored.as_ref().unwrap().engine.max_haystack_len();
        let long = format!("a{}Z", "b".repeat(limit + 1));
        assert!(pattern.search(&long));
        assert_eq!(pattern.slots[1].unwrap().get(), long.len());
    }

    #[test]
    fn captures_own_their_input_and_survive_subsequent_searches() {
        let source = "(?P<optional>x)?(?P<empty>y*)Π";
        let text = Rc::new(Str::new("🌍éΠ"));
        let input = Rc::downgrade(&text);
        // SAFETY: every call uses the declared owners; returned owners are consumed once.
        unsafe {
            let pattern = Rc::from_raw(relar_regex_compile(source.as_ptr(), source.len()));
            let matched = Rc::from_raw(relar_regex_search(Rc::as_ptr(&pattern), Rc::as_ptr(&text)));
            let other = Rc::new(Str::new("xyyΠ"));
            drop(Rc::from_raw(relar_regex_search(
                Rc::as_ptr(&pattern),
                Rc::as_ptr(&other),
            )));
            let miss = Rc::new(Str::new("xyz"));
            assert!(relar_regex_search(Rc::as_ptr(&pattern), Rc::as_ptr(&miss)).is_null());
            drop((pattern, text));
            let m = Rc::as_ptr(&matched);
            assert_eq!(*Rc::from_raw(relar_regex_group(m, 1, 0)), "Π");
            assert!(relar_regex_group(m, 3, 0).is_null());
            let empty = Str::new("empty");
            assert_eq!(
                *Rc::from_raw(relar_regex_group(m, &empty as *const Str as usize, 1)),
                ""
            );
            assert_eq!(
                Int::borrow(relar_regex_bound(m, 1, 0, 0)).as_small(),
                Some(2)
            );
            assert_eq!(
                Int::borrow(relar_regex_bound(m, 1, 0, 1)).as_small(),
                Some(3)
            );
            assert_eq!(
                Int::borrow(relar_regex_bound(m, 3, 0, 1)).as_small(),
                Some(-1)
            );
            assert_eq!(relar_error_pending(), 0);
            let missing = Str::new("missing");
            for (selector, named) in [
                (&missing as *const Str as usize, 1),
                (7, 0),
                (usize::MAX, 0),
            ] {
                assert!(relar_regex_group(m, selector, named).is_null());
                let error = Rc::from_raw(relar_error_take());
                assert_eq!(
                    relar_exception_matches(Rc::as_ptr(&error), Kind::IndexError as u32),
                    1
                );
            }
            assert!(input.upgrade().is_some());
            drop(matched);
            assert!(input.upgrade().is_none());
        }
    }
}
