//! Fixed string patterns and immutable snapshots of participating capture spans.

use crate::StrOwner;
mod captures;

use captures::AnchoredCaptures;
use std::cell::Cell;
use std::ptr;

use memchr::memmem::Finder;
use regex_automata::meta::{self, Regex};
use regex_automata::util::primitives::NonMaxUsize;
use regex_automata::{Anchored, Input, PatternID};
use regex_syntax::hir::{Hir, HirKind, literal::Extractor};

use super::{Error, Int, Kind, Object, Storage, StorageCell, Str, fail, finish, object};

pub(super) struct Pattern {
    spelling: StrOwner,
    regex: Regex,
    cache: meta::Cache,
    slots: Vec<Option<NonMaxUsize>>,
    required: Vec<(Finder<'static>, Option<Vec<u8>>)>,
    anchored: Option<AnchoredCaptures>,
    final_newline_slot: Option<usize>,
    named_groups: Vec<(usize, StrOwner)>,
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

/// A match starting at the first byte must begin with this exact literal.
/// Zero-width conditions may constrain the position further.
fn leading_literal(hir: &Hir) -> Option<&[u8]> {
    match hir.kind() {
        HirKind::Literal(text) => Some(&text.0),
        HirKind::Capture(capture) => leading_literal(&capture.sub),
        HirKind::Concat(parts) => parts
            .iter()
            .find(|part| !matches!(part.kind(), HirKind::Empty | HirKind::Look(_)))
            .and_then(leading_literal),
        _ => None,
    }
}

impl Pattern {
    #[cfg(test)]
    fn new(source: &str) -> Self {
        Self::with_spelling(source, source)
    }

    fn with_spelling(source: &str, spelling: &str) -> Self {
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
        let anchored = if first.is_some() || hir.properties().explicit_captures_len() != 0 {
            AnchoredCaptures::new(&hir, first)
        } else {
            None
        };
        // At least one branch's mandatory substring must exist. An unguarded
        // branch disables the shortcut, as does an unusually wide alternation.
        let branches = match hir.kind() {
            HirKind::Alternation(branches) => branches.as_slice(),
            _ => std::slice::from_ref(&hir),
        };
        let required = branches
            .iter()
            // Translation uses impossible alternatives to preserve Python's branch
            // priority. They cannot produce a match, even on the empty string.
            .filter(|branch| branch.properties().minimum_len().is_some())
            .enumerate()
            .map(|(index, branch)| {
                if index == 4 {
                    return None;
                }
                required_literal(branch)
                    .filter(|s| !s.is_empty() && !(s.len() == 1 && Some(s[0]) == first))
                    .map(|s| {
                        (
                            Finder::new(s).into_owned(),
                            leading_literal(branch).map(<[u8]>::to_vec),
                        )
                    })
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        let final_newline_slot = regex
            .group_info()
            .to_index(PatternID::ZERO, "imre.eol")
            .map(|index| index * 2);
        let named_groups = regex
            .group_info()
            .pattern_names(PatternID::ZERO)
            .enumerate()
            .filter_map(|(index, name)| {
                name.filter(|_| final_newline_slot != Some(index * 2))
                    .map(|name| (index, StrOwner::new(Str::new(name))))
            })
            .collect();
        Self {
            spelling: StrOwner::new(Str::from(spelling.to_owned())),
            regex,
            cache,
            slots,
            anchored,
            required,
            final_newline_slot,
            named_groups,
        }
    }

    /// Expand literal runs and captures for each leftmost, nonempty match.
    /// Searching a range in the original text preserves start/end anchor semantics.
    fn substitute(
        &mut self,
        text: &str,
        template: &[u8],
    ) -> Result<Option<String>, std::collections::TryReserveError> {
        let mut output = String::new();
        let mut consumed = 0;
        let mut append = |part: &str| {
            output.try_reserve(part.len())?;
            output.push_str(part);
            Ok::<_, std::collections::TryReserveError>(())
        };
        loop {
            let input = Input::new(text).span(consumed..text.len());
            if self
                .regex
                .search_slots_with(&mut self.cache, &input, &mut self.slots)
                .is_none()
            {
                break;
            }
            let start = self.slots[0].unwrap().get();
            if let Some(slot) = self.final_newline_slot
                && let Some(nl) = self.slots[slot]
            {
                self.slots[1] = Some(nl);
            }
            append(&text[consumed..start])?;
            let mut program = template;
            while !program.is_empty() {
                let (tag, rest) = program.split_at(4);
                let tag = u32::from_le_bytes(tag.try_into().unwrap());
                if tag & (1 << 31) == 0 {
                    let (literal, rest) = rest.split_at(tag as usize);
                    append(str::from_utf8(literal).expect("validated replacement literal"))?;
                    program = rest;
                } else {
                    let i = ((tag & !(1 << 31)) as usize) * 2;
                    if let Some(a) = self.slots[i] {
                        append(&text[a.get()..self.slots[i + 1].unwrap().get()])?;
                    }
                    program = rest;
                }
            }
            consumed = self.slots[1].unwrap().get();
            debug_assert!(consumed > start);
            if consumed == text.len() {
                break;
            }
        }
        if consumed == 0 {
            Ok(None)
        } else {
            append(&text[consumed..])?;
            Ok(Some(output))
        }
    }

    fn could_match(&self, text: &[u8], at_start: bool) -> bool {
        self.required.is_empty()
            || self
                .required
                .iter()
                .any(|(literal, prefix)| match (at_start, prefix) {
                    (true, Some(prefix)) => text.starts_with(prefix),
                    _ => literal.find(text).is_some(),
                })
    }

    /// The start is already known for match(). A single bounded capture pass
    /// avoids rediscovering the match boundaries before extracting its groups.
    fn match_start(&mut self, text: &str) -> bool {
        if !self.could_match(text.as_bytes(), true) {
            return false;
        }
        let input = Input::new(text).anchored(Anchored::Yes);
        if let Some(bounded) = &mut self.anchored
            && let Ok(found) = bounded.search(&input, &mut self.slots)
        {
            return found.is_some();
        }
        self.regex
            .search_slots_with(&mut self.cache, &input, &mut self.slots)
            .is_some()
    }

    /// Hide the matcher's synthetic final-newline group and restore Python's visible endpoint.
    fn visible_spans(&mut self, text_len: usize, char_start: Option<usize>) -> Spans {
        let mut visible = self.slots.len();
        if let Some(slot) = self.final_newline_slot {
            if let Some(start) = self.slots[slot] {
                self.slots[1] = Some(start);
            }
            visible = slot;
        }
        Spans::new(&self.slots[..visible], text_len, char_start)
    }

    fn search(&mut self, text: &str) -> bool {
        let bytes = text.as_bytes();
        if !self.could_match(bytes, false) {
            return false;
        }
        let mut start = 0;
        if let Some(prefix) = &mut self.anchored
            && let Some(first) = prefix.first
        {
            let Some(at) = memchr::memchr(first, bytes) else {
                return false;
            };
            let input = Input::new(text)
                .span(at..bytes.len())
                .anchored(Anchored::Yes);
            // One anchored attempt avoids the general engine's forward/reverse
            // search on hits. A size-limit error or further candidates fall back
            // to one unanchored search, which preserves the worst-case time bound.
            match prefix.search(&input, &mut self.slots) {
                Ok(Some(_)) => return true,
                Ok(None) => {
                    let Some(next) = memchr::memchr(first, &bytes[at + 1..]) else {
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
    pattern: crate::ObjectOwner,
    text: StrOwner,
    spans: Spans,
}

// Directly storing short capture snapshots must fit an existing Object payload.
const _: () = assert!(size_of::<Match>() <= size_of::<super::mappings::MappingIterator>());

enum Spans {
    Short {
        len: u8,
        slots: [u16; 10],
        // An odd length counts the iterator's character position as well as the paired slots.
        position: Cell<u16>,
    },
    // Iterator matches append their character position after the paired capture slots.
    Long(Box<[Option<NonMaxUsize>]>),
}

impl Spans {
    fn new(slots: &[Option<NonMaxUsize>], text_len: usize, char_start: Option<usize>) -> Self {
        if slots.len() <= 10 && text_len < usize::from(u16::MAX) {
            let mut short = [u16::MAX; 10];
            for (dst, slot) in short.iter_mut().zip(slots) {
                *dst = slot.map_or(u16::MAX, |s| s.get() as u16);
            }
            Self::Short {
                len: slots.len() as u8 + u8::from(char_start.is_some()),
                slots: short,
                position: Cell::new(char_start.unwrap_or_default() as u16),
            }
        } else if let Some(start) = char_start {
            let mut snapshot = Vec::with_capacity(slots.len() + 1);
            snapshot.extend_from_slice(slots);
            snapshot.push(NonMaxUsize::new(start));
            Self::Long(snapshot.into_boxed_slice())
        } else {
            Self::Long(slots.into())
        }
    }

    /// Outer None is an unknown index. Inner None is a known unmatched group.
    fn get(&self, index: usize) -> Option<Option<(usize, usize)>> {
        let at = index.checked_mul(2)?;
        match self {
            Self::Short { len, slots, .. } => {
                if index >= usize::from(*len / 2) {
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
                    .zip(*slots.get(at + 1)?)
                    .map(|(a, b)| (a.get(), b.get())),
            ),
        }
    }

    /// Convert engine offsets from a forward scanner's character position.
    /// A standalone short match tracks the ASCII prefix across repeated bounds.
    fn char_offset(&self, text: &str, byte: usize) -> usize {
        if let Self::Short { len, position, .. } = self
            && len & 1 == 0
        {
            let seen = position.get();
            if seen != u16::MAX {
                if byte <= usize::from(seen) {
                    return byte;
                }
                if text.as_bytes()[usize::from(seen)..byte].is_ascii() {
                    position.set(byte as u16);
                    return byte;
                }
                position.set(u16::MAX);
            }
        }
        let char_start = match self {
            Self::Short { len, position, .. } if len & 1 != 0 => Some(usize::from(position.get())),
            Self::Long(slots) if slots.len() & 1 != 0 => {
                slots.last().copied().flatten().map(NonMaxUsize::get)
            }
            _ => None,
        };
        if let Some(index) = char_start {
            let (start, _) = self.get(0).flatten().expect("a match owns group zero");
            if byte >= start {
                return index + text[start..byte].chars().count();
            }
            return index - text[byte..start].chars().count();
        }
        text[..byte].chars().count()
    }
}

fn handle(storage: Storage) -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(storage))))
}

/// Instantiates a literal already validated by the compiler with this regex engine.
///
/// # Safety
/// `data` holds the original UTF-8 spelling and the translated UTF-8 pattern,
/// separated by 0xff. The compiler has validated the translated pattern.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_literal(data: *const u8, len: usize) -> *const Object {
    // SAFETY: the compiler emits initialized static bytes of this length.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let separator = memchr::memchr(0xff, bytes).expect("missing regex literal separator");
    let (spelling, source) = bytes.split_at(separator);
    // SAFETY: both ranges come from complete Rust strings emitted by the compiler.
    let pattern = unsafe {
        Pattern::with_spelling(
            str::from_utf8_unchecked(&source[1..]),
            str::from_utf8_unchecked(spelling),
        )
    };
    handle(Storage::RegexPattern(Box::new(pattern)))
}

/// Emit the match spans or the gaps before, between and after them.
fn collect_runs(
    len: usize,
    kind: u32,
    split: bool,
    runs: impl Iterator<Item = std::ops::Range<usize>>,
    mut push: impl FnMut(&mut super::Words, std::ops::Range<usize>),
) -> *const Object {
    let mut parts = super::Words::new(kind);
    let mut cursor = 0;
    for span in runs {
        if split {
            push(&mut parts, cursor..span.start);
            cursor = span.end;
        } else {
            push(&mut parts, span);
        }
    }
    if split {
        push(&mut parts, cursor..len);
    }
    crate::ObjectOwner::into_raw(parts.into_object())
}

/// Collect a greedy class whose membership is uniform for all non-ASCII scalars.
///
/// # Safety
/// The caller roots the subject and supplies a 256-byte membership table
/// with the same value for every non-ASCII byte.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_collect_runs(
    value: *const Str,
    members: *const [u8; 256],
    split: u32,
) -> *const Object {
    // SAFETY: the caller owns the subject and the compiler emits all 256 entries.
    let (source, members) = unsafe {
        (
            std::mem::ManuallyDrop::new(StrOwner::from_raw(value)),
            &*members,
        )
    };
    let runs = byte_runs(source.as_bytes(), members);
    collect_runs(source.len(), 3, split != 0, runs, |parts, span| {
        super::text::push_part(parts, &source, &source[span]);
    })
}

fn byte_runs<'a>(
    bytes: &'a [u8],
    members: &'a [u8; 256],
) -> impl Iterator<Item = std::ops::Range<usize>> + 'a {
    let mut runs = imre_regex::ByteRuns::new(members, bytes.len());
    std::iter::from_fn(move || runs.next(bytes))
}

/// Collect binary regex matches or split fields into an owned list of bytes.
///
/// # Safety
/// The caller roots the bytes subject and supplies its 256-entry membership table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_collect_bytes_runs(
    value: *const crate::Bytes,
    members: *const [u8; 256],
    split: u32,
) -> *const Object {
    // SAFETY: both pointers remain live for the whole call.
    let (source, members) = unsafe {
        (
            std::mem::ManuallyDrop::new(crate::BytesOwner::from_raw(value)),
            &*members,
        )
    };
    collect_byte_fields(&source, split != 0, byte_runs(&source, members))
}

fn collect_byte_fields(
    source: &crate::BytesOwner,
    split: bool,
    runs: impl Iterator<Item = std::ops::Range<usize>>,
) -> *const Object {
    collect_runs(source.len(), 8, split, runs, |parts, span| {
        parts
            .values
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
        let field = &source[span];
        let bytes = if field.len() == source.len() {
            crate::BytesOwner::clone(source)
        } else {
            super::bytes::copied_bytes(field)
        };
        parts
            .values
            .push(crate::BytesOwner::into_raw(bytes) as usize);
    })
}

#[derive(Clone, Copy)]
struct CaptureAtom<'a> {
    run: bool,
    captured: bool,
    data: &'a [u8],
}

impl CaptureAtom<'_> {
    fn match_len(self, text: &[u8]) -> usize {
        if self.run {
            text.iter()
                .position(|&byte| self.data[usize::from(byte)] == 0)
                .unwrap_or(text.len())
        } else if text.starts_with(self.data) {
            self.data.len()
        } else {
            0
        }
    }

    fn find(self, text: &[u8]) -> Option<std::ops::Range<usize>> {
        let start = if self.run {
            text.iter().position(|&b| self.data[usize::from(b)] != 0)?
        } else {
            memchr::memmem::find(text, self.data)?
        };
        Some(start..start + self.match_len(&text[start..]))
    }
}

fn capture_atoms(mut program: &[u8]) -> impl Iterator<Item = CaptureAtom<'_>> + Clone {
    std::iter::from_fn(move || {
        let (&flags, rest) = program.split_first()?;
        let (len, rest) = rest.split_first_chunk::<2>().unwrap();
        let (data, next) = rest.split_at(usize::from(u16::from_le_bytes(*len)));
        program = next;
        Some(CaptureAtom {
            run: flags & 1 != 0,
            captured: flags & 2 != 0,
            data,
        })
    })
}

fn collect_rows(
    text: &[u8],
    program: &[u8],
    layout: &super::records::RecordDescriptor,
    field: impl Fn(std::ops::Range<usize>) -> usize,
) -> *const Object {
    let mut atoms = capture_atoms(program);
    let first = atoms.next().unwrap();
    let mut spans = Vec::with_capacity(layout.length);
    let mut batch = super::records::RecordBatch::new(layout);
    let mut rows = super::Words::new(6);
    let mut search = 0;
    'search: while let Some(span) = first.find(&text[search..]) {
        let start = search + span.start;
        let mut end = search + span.end;
        // Every later start in this class run reaches the same suffix. A
        // literal can overlap another occurrence, so retry one byte later.
        search = if first.run { end } else { start + 1 };
        spans.clear();
        if first.captured {
            spans.push(start..end);
        }
        for atom in atoms.clone() {
            let len = atom.match_len(&text[end..]);
            if len == 0 {
                continue 'search;
            }
            if atom.captured {
                spans.push(end..end + len);
            }
            end += len;
        }
        search = end;
        debug_assert_eq!(spans.len(), layout.length);
        rows.values
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
        // SAFETY: the compiler emits a static tuple descriptor with one owned
        // field per capture. Each record owner keeps its allocation alive.
        let row = unsafe {
            match &mut batch {
                Some(batch) => batch.allocate(),
                None => super::records::imre_record_new(layout),
            }
        };
        for (index, span) in spans.drain(..).enumerate() {
            // SAFETY: every compulsory capture fills one distinct field of the
            // descriptor's string or bytes kind before publishing the tuple.
            unsafe { row.add(index).write(field(span)) };
        }
        rows.values.push(row as usize);
    }
    crate::ObjectOwner::into_raw(rows.into_object())
}

/// Collect the compulsory fields of an unambiguous concatenation.
///
/// # Safety
/// The compiler supplies the capture program and a static tuple descriptor
/// with the matching field kinds. The caller roots the indicated subject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_collect_rows(
    value: *const std::ffi::c_void,
    program: *const u8,
    len: usize,
    layout: *const super::records::RecordDescriptor,
    bytes: u32,
) -> *const Object {
    // SAFETY: these pointers and lengths come from immutable compiler metadata.
    let (program, layout) = unsafe { (std::slice::from_raw_parts(program, len), &*layout) };
    if bytes != 0 {
        // SAFETY: the bytes flag identifies a rooted bytes owner.
        let source =
            unsafe { std::mem::ManuallyDrop::new(crate::BytesOwner::from_raw(value.cast())) };
        collect_rows(&source, program, layout, |span| {
            crate::BytesOwner::into_raw(super::bytes::copied_bytes(&source[span])) as usize
        })
    } else {
        // SAFETY: the clear flag identifies a rooted string owner.
        let source = unsafe { std::mem::ManuallyDrop::new(StrOwner::from_raw(value.cast())) };
        collect_rows(source.as_bytes(), program, layout, |span| {
            StrOwner::into_raw(super::text::owned_string(&source[span])) as usize
        })
    }
}

/// Search the ordered literal payload for the next leftmost, nonempty match.
fn literal_ranges<'a>(
    text: &'a [u8],
    program: &'a [u8],
) -> impl Iterator<Item = std::ops::Range<usize>> + 'a {
    let (heads, rest) = program.split_at(4);
    let (offsets, entries) = rest.split_at(257 * 2);
    let (offsets, _) = offsets.as_chunks::<2>();
    let mut at = 0;
    std::iter::from_fn(move || {
        loop {
            let tail = &text[at..];
            at += match heads[0] {
                1 => memchr::memchr(heads[1], tail),
                2 => memchr::memchr2(heads[1], heads[2], tail),
                3 => memchr::memchr3(heads[1], heads[2], heads[3], tail),
                _ => tail
                    .iter()
                    .position(|&b| offsets[usize::from(b)] != offsets[usize::from(b) + 1]),
            }?;
            let group = usize::from(text[at]);
            let start = usize::from(u16::from_le_bytes(offsets[group]));
            let end = usize::from(u16::from_le_bytes(offsets[group + 1]));
            let mut candidates = &entries[start..end];
            while let Some((&len, rest)) = candidates.split_first() {
                let (literal, next) = rest.split_at(usize::from(len));
                if text[at..].starts_with(literal) {
                    let start = at;
                    at += literal.len();
                    return Some(start..at);
                }
                candidates = next;
            }
            at += 1;
        }
    })
}

/// Collect matches of complete ordered literals or the fields between matches.
///
/// # Safety
/// The compiler supplies the complete immutable literal program. The caller
/// roots a bytes owner when bit one of `flags` is set, otherwise a string owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_collect_literals(
    value: *const std::ffi::c_void,
    program: *const u8,
    len: usize,
    flags: u32,
) -> *const Object {
    // SAFETY: the compiler supplies the matching constant address and length.
    let program = unsafe { std::slice::from_raw_parts(program, len) };
    let split = flags & 1 != 0;
    if flags & 2 != 0 {
        // SAFETY: the bytes flag identifies a rooted bytes owner.
        let source =
            unsafe { std::mem::ManuallyDrop::new(crate::BytesOwner::from_raw(value.cast())) };
        collect_byte_fields(&source, split, literal_ranges(&source, program))
    } else {
        // SAFETY: a clear bytes flag identifies a rooted string owner.
        let source = unsafe { std::mem::ManuallyDrop::new(StrOwner::from_raw(value.cast())) };
        collect_runs(
            source.len(),
            3,
            split,
            literal_ranges(source.as_bytes(), program),
            |parts, span| {
                super::text::push_part(parts, &source, &source[span]);
            },
        )
    }
}

/// Collect a greedy Unicode class using sorted membership change points.
///
/// # Safety
/// The caller roots the subject and supplies the complete immutable program
/// from imre_regex::run_class, including its first 256 byte entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_collect_unicode_runs(
    value: *const Str,
    class: *const u8,
    len: usize,
    split: u32,
) -> *const Object {
    // SAFETY: the caller owns the subject and the compiler supplies the program length.
    let (source, class) = unsafe {
        (
            std::mem::ManuallyDrop::new(StrOwner::from_raw(value)),
            std::slice::from_raw_parts(class, len),
        )
    };
    let (members, ranges) = class.split_at(256);
    let (bounds, _) = ranges.as_chunks::<4>();
    let member = |ch: char| {
        if ch.is_ascii() {
            members[ch as usize] != 0
        } else {
            bounds.partition_point(|bound| u32::from_le_bytes(*bound) <= ch as u32) % 2 == 1
        }
    };
    let mut chars = source.char_indices();
    let runs = std::iter::from_fn(|| {
        let (start, _) = chars.find(|&(_, ch)| member(ch))?;
        let end = chars
            .find(|&(_, ch)| !member(ch))
            .map_or(source.len(), |(i, _)| i);
        Some(start..end)
    });
    collect_runs(source.len(), 3, split != 0, runs, |parts, span| {
        super::text::push_part(parts, &source, &source[span]);
    })
}

/// Return the original Python spelling even when the native engine expanded an escape.
///
/// # Safety
/// `pattern` must be a rooted compiled pattern.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_spelling(pattern: *const Object) -> *const Str {
    // SAFETY: the caller keeps the pattern alive throughout the borrow.
    unsafe {
        object(pattern).0.with(|storage| {
            let Storage::RegexPattern(pattern) = storage else {
                unreachable!()
            };
            StrOwner::into_raw(pattern.spelling.clone())
        })
    }
}

/// Compile an exception assertion's runtime string with the same translator as
/// fixed literals. Never reinterpret an unsupported pattern as literal text.
///
/// # Safety
/// `spelling` must point to a rooted Python string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_unittest_compile_pattern(spelling: *const Str) -> *const Object {
    // SAFETY: the caller retains its string for this call.
    let spelling = unsafe { &*spelling };
    match imre_regex::translate(spelling, false) {
        Ok(pattern) => handle(Storage::RegexPattern(Box::new(Pattern::with_spelling(
            &pattern.source,
            spelling,
        )))),
        Err(reason) => {
            Error::new(
                Kind::NotImplementedError,
                format!("unsupported native exception regex: {reason}"),
            )
            .raise();
            ptr::null()
        }
    }
}

/// Compile a Python pattern supplied at runtime, then search for a match.
///
/// # Safety
/// The caller roots both Python strings until the match snapshot has been produced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_search_text(
    spelling: *const Str,
    text: *const Str,
    anchored: bool,
    verbose: bool,
) -> *const Object {
    // SAFETY: the caller retains the string for the full call.
    let spelling = unsafe { &*spelling };
    let compiled = match imre_regex::translate(spelling, verbose) {
        Ok(pattern) => Pattern::with_spelling(&pattern.source, spelling),
        Err(reason) => {
            Error::new(
                Kind::NotImplementedError,
                format!("unsupported native regex: {reason}"),
            )
            .raise();
            return ptr::null();
        }
    };
    let pattern = crate::ObjectOwner::new(Object(StorageCell::new(Storage::RegexPattern(
        Box::new(compiled),
    ))));
    // SAFETY: the local owner and caller respectively root the pattern and text.
    unsafe { match_text(crate::ObjectOwner::as_ptr(&pattern), text, anchored) }
}

/// Substitute all matches; a miss retains the original string.
///
/// # Safety
/// Root the compiled nonempty pattern and text. The compiler supplies an encoded
/// replacement of exactly `len` bytes, with valid capture indices and UTF-8 runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_sub(
    pattern: *const Object,
    text: *const Str,
    template: *const u8,
    len: usize,
) -> *const Str {
    // SAFETY: the static template is initialized even for an empty replacement;
    // neither the matching engine nor substitution can reenter the pattern.
    unsafe {
        let template = std::slice::from_raw_parts(template, len);
        object(pattern).0.with_mut(|storage| {
            let Storage::RegexPattern(compiled) = storage else {
                unreachable!()
            };
            match compiled.substitute(&*text, template) {
                Ok(Some(output)) => StrOwner::into_raw(StrOwner::new(Str::from(output))),
                Ok(None) => StrOwner::into_raw(crate::StrOwner::acquire_word(text as usize)),
                Err(_) => {
                    super::exceptions::sequence_allocation_failed();
                    ptr::null()
                }
            }
        })
    }
}

/// Returns a rooted snapshot of the leftmost match, or a null optional on a miss.
///
/// # Safety
/// Both arguments are rooted handles of the declared types. The search cannot reenter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_search(
    pattern: *const Object,
    text: *const Str,
) -> *const Object {
    // SAFETY: both published entry points have the same rooted arguments.
    unsafe { match_text(pattern, text, false) }
}

/// Match a fixed pattern at the start of the string.
///
/// # Safety
/// The compiled pattern and string must both be rooted for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_match(
    pattern: *const Object,
    text: *const Str,
) -> *const Object {
    // SAFETY: the arguments remain rooted until the match snapshot is complete.
    unsafe { match_text(pattern, text, true) }
}

unsafe fn match_text(pattern: *const Object, text: *const Str, anchored: bool) -> *const Object {
    // SAFETY: both inputs outlive the search. Mutating engine scratch cannot call
    // user code, and only copied capture spans escape this private storage borrow.
    unsafe {
        object(pattern).0.with_mut(|storage| {
            let Storage::RegexPattern(compiled) = storage else {
                fail("SystemError", "expected a compiled regex");
            };
            let found = if anchored {
                compiled.match_start((&*text).as_str())
            } else {
                compiled.search(&*text)
            };
            if !found {
                return ptr::null();
            }
            let spans = compiled.visible_spans((&*text).len(), None);
            handle(Storage::RegexMatch(Match {
                pattern: crate::ObjectOwner::acquire_word(pattern as usize),
                text: crate::StrOwner::acquire_word(text as usize),
                spans,
            }))
        })
    }
}

/// A one-shot cursor over a statically nonempty pattern; each match snapshots
/// the group spans and retains the original text independently of the cursor.
pub(super) struct FindIter {
    pattern: crate::ObjectOwner,
    text: StrOwner,
    // NonMaxUsize leaves room for the exhausted state in the byte offset.
    position: Option<(NonMaxUsize, usize)>,
}

impl FindIter {
    pub(super) fn next(&mut self) -> Option<(super::OwnedWord, Option<super::OwnedWord>)> {
        let (offset, char_offset) = self.position.take()?;
        let offset = offset.get();
        // SAFETY: this cursor owns a distinct pattern; a search cannot reenter or mutate the cursor.
        unsafe {
            self.pattern.0.with_mut(|storage| {
                let Storage::RegexPattern(compiled) = storage else {
                    unreachable!()
                };
                let input = Input::new(self.text.as_str()).span(offset..self.text.len());
                compiled.regex.search_slots_with(
                    &mut compiled.cache,
                    &input,
                    &mut compiled.slots,
                )?;
                let start = compiled.slots[0].unwrap().get();
                let char_start = char_offset + self.text[offset..start].chars().count();
                let spans = compiled.visible_spans(self.text.len(), Some(char_start));
                let end = compiled.slots[1].unwrap();
                debug_assert!(start < end.get());
                self.position = (end.get() < self.text.len()).then(|| {
                    (
                        end,
                        char_start + self.text[start..end.get()].chars().count(),
                    )
                });
                let found = handle(Storage::RegexMatch(Match {
                    pattern: crate::ObjectOwner::clone(&self.pattern),
                    text: StrOwner::clone(&self.text),
                    spans,
                }));
                // SAFETY: the match transfers one object owner to the shared cursor ABI.
                Some((
                    super::OwnedWord::from_owned(found as usize, super::words::WordKind::new(5)),
                    None,
                ))
            })
        }
    }
}

/// Iterate a proved nonempty pattern without materializing the matches.
///
/// # Safety
/// Both pointers must be rooted native objects of the indicated types.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_finditer(
    pattern: *const Object,
    text: *const Str,
) -> *const Object {
    handle(Storage::RegexIterator(FindIter {
        // SAFETY: each input remains rooted until the cursor retains its own owners.
        pattern: unsafe { crate::ObjectOwner::acquire_word(pattern as usize) },
        // SAFETY: the original text is live through this acquisition.
        text: unsafe { crate::StrOwner::acquire_word(text as usize) },
        position: Some((NonMaxUsize::new(0).unwrap(), 0)),
    }))
}

impl Match {
    pub(super) fn write_repr(&self, output: &mut impl std::fmt::Write) -> std::fmt::Result {
        let (start, end) = self
            .spans
            .get(0)
            .flatten()
            .expect("a match owns group zero");
        let mut text = String::new();
        super::exceptions::write_quoted_repr(&self.text[start..end], &mut text)?;
        // CPython's %.50R precision applies to the quoted spelling in code points.
        if let Some((cut, _)) = text.char_indices().nth(50) {
            text.truncate(cut);
        }
        write!(
            output,
            "<re.Match object; span=({}, {}), match={text}>",
            self.spans.char_offset(&self.text, start),
            self.spans.char_offset(&self.text, end),
        )
    }

    /// Empty captures own an empty string. Only nonparticipating groups return null.
    fn substring(&self, span: Option<(usize, usize)>) -> *const Str {
        match span {
            Some((0, end)) if end == self.text.len() => {
                StrOwner::into_raw(StrOwner::clone(&self.text))
            }
            Some((start, end)) => {
                StrOwner::into_raw(StrOwner::new(Str::new(&self.text[start..end])))
            }
            None => ptr::null(),
        }
    }

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
pub unsafe extern "C" fn imre_regex_group(
    value: *const Object,
    selector: usize,
    named: u32,
) -> *const Str {
    // SAFETY: the match and selector are rooted; no borrowed substring escapes.
    let result = unsafe {
        object(value).0.with(|storage| {
            let matched = storage.regex_match();
            Ok(matched.substring(matched.span(selector, named)?))
        })
    };
    finish(result, ptr::null())
}

/// Copies the named captures in declaration order into a fresh mutable mapping.
///
/// # Safety
/// The caller roots a live match. It owns its pattern and matched text throughout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_groupdict(value: *const Object) -> *const Object {
    use super::mappings::Mapping;
    use super::words::{NULLABLE, OwnedWord, WordKind};
    let key_kind = WordKind::new(3);
    let value_kind = WordKind::new(3 | NULLABLE);
    let mut entries = Mapping::new(key_kind.raw(), value_kind.raw());
    // SAFETY: immutable match and pattern borrows cannot overlap a mutation.
    // Each inserted word transfers exactly the owned Rc produced below, or None.
    let inserted = unsafe {
        object(value).0.with(|storage| {
            let matched = storage.regex_match();
            matched.pattern.0.with(|storage| {
                let Storage::RegexPattern(pattern) = storage else {
                    unreachable!()
                };
                // Frozen capture names belong to the pattern. Each fresh
                // dictionary retains those same immutable keys.
                for (index, name) in &pattern.named_groups {
                    let key = StrOwner::into_raw(StrOwner::clone(name)) as usize;
                    let capture = matched.substring(matched.spans.get(*index).unwrap()) as usize;
                    entries.insert_unique(
                        OwnedWord::from_owned(key, key_kind),
                        OwnedWord::from_owned(capture, value_kind),
                    )?;
                }
                Ok(())
            })
        })
    };
    crate::finish(
        inserted.map(|()| {
            handle(Storage::Dictionary(
                crate::objects::mappings::TableOwner::new(entries),
            ))
        }),
        std::ptr::null(),
    )
}

/// Checks participation without allocating the capture substring.
///
/// # Safety
/// Root the same Match and selector types as `imre_regex_group`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_present(
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
/// Root the same match and selector types as `imre_regex_group`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_regex_bound(
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
    use crate::objects::exceptions::{imre_error_pending, imre_error_take, imre_exception_matches};

    #[test]
    fn wide_and_long_capture_snapshots_preserve_exact_offsets() {
        let spelling = r"(\x61)(b)?(c)?(d)?(e)?(?P<last>f)";
        let pattern = "(a)(b)?(c)?(d)?(e)?(?P<last>f)";
        for text in ["af".to_owned(), format!("{}af", "é".repeat(40_000))] {
            let prefix_chars = text.chars().count() - 2;
            let text = StrOwner::new(Str::from(text));
            // SAFETY: the helper owns each ABI handle until its last access.
            unsafe {
                let bytes = [spelling.as_bytes(), &[0xff], pattern.as_bytes()].concat();
                let compiled =
                    crate::ObjectOwner::from_raw(imre_regex_literal(bytes.as_ptr(), bytes.len()));
                let original =
                    StrOwner::from_raw(imre_regex_spelling(crate::ObjectOwner::as_ptr(&compiled)));
                assert_eq!(original.as_str(), spelling);
                let found = crate::ObjectOwner::from_raw(imre_regex_search(
                    crate::ObjectOwner::as_ptr(&compiled),
                    StrOwner::as_ptr(&text),
                ));
                let m = crate::ObjectOwner::as_ptr(&found);
                let last = Str::new("last");
                assert_eq!(
                    Int::borrow(imre_regex_bound(m, 1, 0, 0)).as_small(),
                    Some(prefix_chars as i64)
                );
                assert_eq!(
                    Int::borrow(imre_regex_bound(m, 13, 0, 1)).as_small(),
                    Some(prefix_chars as i64 + 2)
                );
                assert_eq!(
                    *StrOwner::from_raw(imre_regex_group(m, &last as *const Str as usize, 1)),
                    "f"
                );
                assert_eq!(imre_regex_present(m, 5, 0), 0);
                assert_eq!(imre_regex_present(m, 13, 0), 1);
                assert_eq!(imre_regex_present(m, 15, 0), 0);
                let error = crate::ObjectOwner::from_raw(imre_error_take());
                assert_eq!(
                    imre_exception_matches(
                        crate::ObjectOwner::as_ptr(&error),
                        Kind::IndexError as u32
                    ),
                    1
                );
            }
        }
    }

    #[test]
    fn impossible_alternatives_do_not_disable_required_literals() {
        let mut pattern = Pattern::new(r"^(one.+)|\P{any}|(.* two .*)|\P{any}|(three.*)");
        assert!(!pattern.could_match(b"ordinary line", true));
        for text in ["one!", "a two b", "three"] {
            assert!(pattern.match_start(text));
        }

        let mut empty = Pattern::new(r"^(one.+)|\P{any}|()");
        assert!(empty.match_start("ordinary line"));
    }

    #[test]
    fn failed_first_prefix_and_bounded_engine_limit_fall_back() {
        let mut pattern = Pattern::new("a[ab]+Z");
        assert!(pattern.search("a!abc abbaZ"));
        assert_eq!(pattern.slots[0].unwrap().get(), 6);
        let limit = pattern.anchored.as_ref().unwrap().max_haystack_len();
        let long = format!("a{}Z", "b".repeat(limit + 1));
        assert!(pattern.search(&long));
        assert_eq!(pattern.slots[1].unwrap().get(), long.len());
    }

    #[test]
    fn anchored_captures_without_a_fixed_prefix_match_the_general_engine() {
        let mut pattern = Pattern::new(r"(?P<sign>[+-])? *(?P<code>[A-Z]+):(.+)");
        let bounded = pattern.anchored.as_ref().unwrap();
        assert_eq!(bounded.first, None);
        let long = format!("B:{}", "x".repeat(bounded.max_haystack_len() + 1));
        for text in ["-  XY:é", "B:x", "bad A:x", long.as_str()] {
            let mut expected = vec![None; pattern.slots.len()];
            let found = pattern
                .regex
                .search_slots_with(
                    &mut pattern.cache,
                    &Input::new(text).anchored(Anchored::Yes),
                    &mut expected,
                )
                .is_some();
            assert_eq!(pattern.match_start(text), found);
            if found {
                assert_eq!(pattern.slots, expected);
            }
        }
    }

    #[test]
    fn captures_own_their_input_and_survive_subsequent_searches() {
        let source = "(?P<optional>x)?(?P<empty>y*)Π";
        let text = StrOwner::new(Str::new("🌍éΠ"));
        let input = StrOwner::downgrade(&text);
        // SAFETY: every call uses the declared owners; returned owners are consumed once.
        unsafe {
            let pattern = crate::ObjectOwner::from_raw(handle(Storage::RegexPattern(Box::new(
                Pattern::new(source),
            ))));
            let matched = crate::ObjectOwner::from_raw(imre_regex_search(
                crate::ObjectOwner::as_ptr(&pattern),
                StrOwner::as_ptr(&text),
            ));
            let other = StrOwner::new(Str::new("xyyΠ"));
            drop(crate::ObjectOwner::from_raw(imre_regex_search(
                crate::ObjectOwner::as_ptr(&pattern),
                StrOwner::as_ptr(&other),
            )));
            let miss = StrOwner::new(Str::new("xyz"));
            assert!(
                imre_regex_search(
                    crate::ObjectOwner::as_ptr(&pattern),
                    StrOwner::as_ptr(&miss)
                )
                .is_null()
            );
            drop((pattern, text));
            let m = crate::ObjectOwner::as_ptr(&matched);
            assert_eq!(*StrOwner::from_raw(imre_regex_group(m, 1, 0)), "Π");
            assert!(imre_regex_group(m, 3, 0).is_null());
            let empty = Str::new("empty");
            assert_eq!(
                *StrOwner::from_raw(imre_regex_group(m, &empty as *const Str as usize, 1)),
                ""
            );
            assert_eq!(
                Int::borrow(imre_regex_bound(m, 1, 0, 0)).as_small(),
                Some(2)
            );
            assert_eq!(
                Int::borrow(imre_regex_bound(m, 1, 0, 1)).as_small(),
                Some(3)
            );
            assert_eq!(
                Int::borrow(imre_regex_bound(m, 3, 0, 1)).as_small(),
                Some(-1)
            );
            assert_eq!(imre_error_pending(), 0);
            let missing = Str::new("missing");
            for (selector, named) in [
                (&missing as *const Str as usize, 1),
                (7, 0),
                (usize::MAX, 0),
            ] {
                assert!(imre_regex_group(m, selector, named).is_null());
                let error = crate::ObjectOwner::from_raw(imre_error_take());
                assert_eq!(
                    imre_exception_matches(
                        crate::ObjectOwner::as_ptr(&error),
                        Kind::IndexError as u32
                    ),
                    1
                );
            }
            assert!(input.upgrade().is_some());
            drop(matched);
            assert!(input.upgrade().is_none());
        }
    }
}
