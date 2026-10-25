//! Python 3.12's default line differ. Both the line alignment and the optional
//! intraline guides use the same longest-contiguous-match rule.

use super::{Str, Words, python_whitespace};
use crate::StrOwner;
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ops::Range;

/// A view into a rooted string-list buffer. Each word is already a string
/// handle, so the sorted merge needs no temporary array of fat string slices.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Line<'a>(usize, std::marker::PhantomData<&'a Str>);

impl<'a> Line<'a> {
    /// The caller guarantees that every word is a nonnullable string owner.
    unsafe fn from_words(words: &'a Words) -> &'a [Self] {
        debug_assert_eq!(words.kind.raw(), 3);
        // SAFETY: Line is one address word. Elements remain rooted by Words.
        unsafe { std::slice::from_raw_parts(words.values.as_ptr().cast(), words.values.len()) }
    }

    fn text(self) -> &'a str {
        // SAFETY: only from_words constructs these views, from a rooted string list.
        unsafe { (&*(self.0 as *const Str)).as_str() }
    }
}

#[derive(Clone, Copy, Default)]
struct Block {
    left: usize,
    right: usize,
    len: usize,
}

struct Matcher<'a, T> {
    right: &'a [T],
    positions: HashMap<T, SmallVec<[usize; 1]>>,
    junk: fn(T) -> bool,
}

impl<'a, T: Copy + Eq + Hash> Matcher<'a, T> {
    fn new(right: &'a [T], junk: fn(T) -> bool) -> Self {
        let mut positions = HashMap::<T, SmallVec<[usize; 1]>>::with_capacity(right.len());
        for (index, &item) in right.iter().enumerate() {
            positions.entry(item).or_default().push(index);
        }
        Self {
            right,
            positions,
            junk,
        }
    }

    /// Splits around each longest match, then coalesces adjacent blocks. Junk
    /// and popular elements can extend a match even when they cannot start one.
    fn blocks(&self, left: &[T]) -> Vec<Block> {
        let right = self.right;
        let limit = if right.len() < 200 {
            usize::MAX
        } else {
            right.len() / 100 + 1
        };
        let positions: Vec<&[usize]> = left
            .iter()
            .map(|item| {
                self.positions
                    .get(item)
                    .filter(|hits| hits.len() <= limit && !(self.junk)(*item))
                    .map_or(&[][..], |hits| hits.as_slice())
            })
            .collect();
        let mut previous = vec![(0, 0); right.len()];
        let mut regions = vec![(0..left.len(), 0..right.len())];
        let mut blocks = Vec::new();
        while let Some((a, b)) = regions.pop() {
            previous[b.clone()].fill((0, 0));
            let mut best = Block {
                left: a.start,
                right: b.start,
                len: 0,
            };
            for i in a.clone() {
                let hits = positions[i];
                let low = hits.partition_point(|&j| j < b.start);
                let high = hits.partition_point(|&j| j < b.end);
                // Writing right to left keeps the previous row's j-1 available.
                // Among equal lengths on this row Python chooses the earliest j.
                for &j in hits[low..high].iter().rev() {
                    let n = if j > b.start && previous[j - 1].0 == i {
                        previous[j - 1].1 + 1
                    } else {
                        1
                    };
                    previous[j] = (i + 1, n);
                    if n > best.len
                        || (n == best.len && i + 1 - n == best.left && j + 1 - n < best.right)
                    {
                        best = Block {
                            left: i + 1 - n,
                            right: j + 1 - n,
                            len: n,
                        };
                    }
                }
            }
            for junk in [false, true] {
                while best.left > a.start
                    && best.right > b.start
                    && (self.junk)(right[best.right - 1]) == junk
                    && left[best.left - 1] == right[best.right - 1]
                {
                    best.left -= 1;
                    best.right -= 1;
                    best.len += 1;
                }
                while best.left + best.len < a.end
                    && best.right + best.len < b.end
                    && (self.junk)(right[best.right + best.len]) == junk
                    && left[best.left + best.len] == right[best.right + best.len]
                {
                    best.len += 1;
                }
            }
            if best.len == 0 {
                continue;
            }
            blocks.push(best);
            if a.start < best.left && b.start < best.right {
                regions.push((a.start..best.left, b.start..best.right));
            }
            if best.left + best.len < a.end && best.right + best.len < b.end {
                regions.push((best.left + best.len..a.end, best.right + best.len..b.end));
            }
        }
        blocks.sort_unstable_by_key(|block| block.left);
        let mut merged: Vec<Block> = Vec::with_capacity(blocks.len() + 1);
        for block in blocks {
            if let Some(last) = merged.last_mut()
                && last.left + last.len == block.left
                && last.right + last.len == block.right
            {
                last.len += block.len;
            } else {
                merged.push(block);
            }
        }
        merged.push(Block {
            left: left.len(),
            right: right.len(),
            len: 0,
        });
        merged
    }
}

fn char_junk(value: char) -> bool {
    matches!(value, ' ' | '\t')
}

struct CharLine {
    chars: Vec<char>,
    counts: HashMap<char, usize>,
}

impl CharLine {
    fn new(line: &str) -> Self {
        let chars: Vec<_> = line.chars().collect();
        let mut counts = HashMap::new();
        for &ch in &chars {
            *counts.entry(ch).or_insert(0) += 1;
        }
        Self { chars, counts }
    }
}

/// Finds the highest-scoring nonidentical pair, visiting the right sequence
/// first for Python's tie order. An identical junk/popular line is the fallback.
fn synchronization(left: &[Line], right: &[Line]) -> Option<(usize, usize, Vec<Block>)> {
    let left_chars: Vec<_> = left.iter().map(|line| CharLine::new(line.text())).collect();
    let mut identical = None;
    let mut best = None;
    let mut best_ratio = 0.74;
    for (j, line) in right.iter().map(|line| line.text()).enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let matcher = Matcher::new(&chars, char_junk);
        for (i, other) in left_chars.iter().enumerate() {
            if left[i].text() == line {
                identical.get_or_insert((i, j, Vec::new()));
                continue;
            }
            // Nonidentical strings have a positive total length. Length and
            // multiset overlap bound the score without running the alignment.
            let total = (other.chars.len() + chars.len()) as f64;
            let ratio = |matches: usize| 2.0 * matches as f64 / total;
            if ratio(other.chars.len().min(chars.len())) <= best_ratio {
                continue;
            }
            let common: usize = other
                .counts
                .iter()
                .map(|(ch, &count)| {
                    count.min(matcher.positions.get(ch).map_or(0, |hits| hits.len()))
                })
                .sum();
            if ratio(common) <= best_ratio {
                continue;
            }
            let blocks = matcher.blocks(&other.chars);
            let matches = blocks.iter().map(|b| b.len).sum();
            let score = ratio(matches);
            if score > best_ratio {
                best_ratio = score;
                best = Some((i, j, blocks));
            }
        }
    }
    if best_ratio >= 0.75 { best } else { identical }
}

fn marked_pair(left: &str, right: &str, blocks: Vec<Block>, emit: &mut impl FnMut(&str, &str)) {
    if left == right {
        emit("  ", left);
        return;
    }
    let l: Vec<_> = left.chars().collect();
    let mut ltags = String::new();
    let mut rtags = String::new();
    let mut li = 0;
    let mut ri = 0;
    for block in blocks {
        let replaced = li < block.left && ri < block.right;
        ltags.extend(std::iter::repeat_n(
            if replaced { '^' } else { '-' },
            block.left - li,
        ));
        rtags.extend(std::iter::repeat_n(
            if replaced { '^' } else { '+' },
            block.right - ri,
        ));
        for &ch in &l[block.left..block.left + block.len] {
            let marker = if python_whitespace(ch) { ch } else { ' ' };
            ltags.push(marker);
            rtags.push(marker);
        }
        li = block.left + block.len;
        ri = block.right + block.len;
    }
    for (tag, source, guide) in [("- ", left, ltags), ("+ ", right, rtags)] {
        emit(tag, source);
        let trimmed = guide.trim_end_matches(python_whitespace);
        if !trimmed.is_empty() {
            let mut hint = String::with_capacity(trimmed.len() + 1);
            hint.push_str(trimmed);
            hint.push('\n');
            emit("? ", &hint);
        }
    }
}

fn dump(lines: &[Line], tag: &str, emit: &mut impl FnMut(&str, &str)) {
    for line in lines {
        emit(tag, line.text());
    }
}

/// Expands replacement regions on an explicit stack. Emitting the shorter
/// unmatched side first is observable when that side is the insertion.
fn changed(left: &[Line], right: &[Line], emit: &mut impl FnMut(&str, &str)) {
    if left.is_empty() {
        dump(right, "+ ", emit);
        return;
    }
    if right.is_empty() {
        dump(left, "- ", emit);
        return;
    }
    enum Work {
        Region(Range<usize>, Range<usize>),
        Pair(usize, usize, Vec<Block>),
    }
    let mut pending = vec![Work::Region(0..left.len(), 0..right.len())];
    while let Some(work) = pending.pop() {
        let (a, b) = match work {
            Work::Region(a, b) => (a, b),
            Work::Pair(i, j, blocks) => {
                marked_pair(left[i].text(), right[j].text(), blocks, emit);
                continue;
            }
        };
        if !a.is_empty()
            && !b.is_empty()
            && let Some((i, j, blocks)) = synchronization(&left[a.clone()], &right[b.clone()])
        {
            let i = i + a.start;
            let j = j + b.start;
            pending.push(Work::Region(i + 1..a.end, j + 1..b.end));
            pending.push(Work::Pair(i, j, blocks));
            pending.push(Work::Region(a.start..i, b.start..j));
        } else if b.len() < a.len() {
            dump(&right[b], "+ ", emit);
            dump(&left[a], "- ", emit);
        } else {
            dump(&left[a], "- ", emit);
            dump(&right[b], "+ ", emit);
        }
    }
}

/// In sorted sequences, common runs with equal multiplicity have a single
/// alignment that matches the whole run. Every longest block must use it:
/// an offset inside such a run can only shorten the block. Unequal counts
/// or autojunk can change Python's choice, so those inputs use the matcher.
fn sorted_line_blocks(left: &[Line], right: &[Line]) -> Option<Vec<Block>> {
    let mut blocks: Vec<Block> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        match left[i].text().cmp(right[j].text()) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                let line = left[i].text();
                let n = left[i..]
                    .iter()
                    .take_while(|item| item.text() == line)
                    .count();
                let m = right[j..]
                    .iter()
                    .take_while(|item| item.text() == line)
                    .count();
                if n != m || (right.len() >= 200 && m > right.len() / 100 + 1) {
                    return None;
                }
                if let Some(last) = blocks.last_mut()
                    && last.left + last.len == i
                    && last.right + last.len == j
                {
                    last.len += n;
                } else {
                    blocks.push(Block {
                        left: i,
                        right: j,
                        len: n,
                    });
                }
                i += n;
                j += n;
            }
        }
    }
    blocks.push(Block {
        left: left.len(),
        right: right.len(),
        len: 0,
    });
    Some(blocks)
}

/// With no identical line in a replacement, every input survives with its own
/// sign and in its original side's order. Interleaving the two sides cannot
/// affect a stable sort by the entire untagged line: ties have the same sign.
fn disjoint(left: &[Line], right: &[Line]) -> bool {
    let (small, large) = if left.len() < right.len() {
        (left, right)
    } else {
        (right, left)
    };
    let keys: HashSet<&str> = small.iter().map(|line| line.text()).collect();
    large.iter().all(|line| !keys.contains(line.text()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Output {
    Full,
    ProjectedChanges,
}

fn ndiff(
    left: &[Line],
    right: &[Line],
    sorted: Option<Vec<Block>>,
    output: Output,
    mut emit: impl FnMut(&str, &str),
) {
    let (mut i, mut j) = (0, 0);
    let exact_common_runs = sorted.is_some();
    let blocks = sorted.unwrap_or_else(|| {
        let a: Vec<_> = left.iter().map(|line| line.text()).collect();
        let b: Vec<_> = right.iter().map(|line| line.text()).collect();
        Matcher::new(&b, |_| false).blocks(&a)
    });
    for block in blocks {
        let removed = &left[i..block.left];
        let added = &right[j..block.right];
        if output == Output::ProjectedChanges
            && (exact_common_runs
                || removed.is_empty()
                || added.is_empty()
                || disjoint(removed, added))
        {
            dump(removed, "- ", &mut emit);
            dump(added, "+ ", &mut emit);
        } else {
            changed(removed, added, &mut emit);
        }
        i = block.left + block.len;
        j = block.right + block.len;
        if output == Output::Full {
            dump(&left[block.left..i], "  ", &mut emit);
        }
    }
}

/// Builds a full diff or the signed projection from two immutable snapshots.
///
/// # Safety
/// Each buffer owns nonnullable strings for the duration of the call.
pub(super) unsafe fn materialize_words(left: &Words, right: &Words, output: Output) -> Words {
    let ordered = left.strings_ascending() && right.strings_ascending();
    // SAFETY: both input buffers remain rooted and immutable while their views are live.
    let (left, right) = unsafe { (Line::from_words(left), Line::from_words(right)) };
    let sorted = if ordered {
        sorted_line_blocks(left, right)
    } else {
        None
    };
    let mut words = Words::new(3);
    if output == Output::Full {
        words.values.reserve(left.len().max(right.len()));
    }
    ndiff(left, right, sorted, output, |prefix, line| {
        if output == Output::ProjectedChanges && !matches!(prefix, "- " | "+ ") {
            return;
        }
        let mut tagged = Str::with_capacity(prefix.len() + line.len());
        tagged.push_str(prefix);
        tagged.push_str(line);
        words
            .values
            .push(StrOwner::into_raw(StrOwner::new(tagged)) as usize);
    });
    words.update_header();
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(lines: &[&str]) -> Words {
        let mut words = Words::new(3);
        for &line in lines {
            words
                .values
                .push(StrOwner::into_raw(StrOwner::new(Str::from(line))) as usize);
        }
        words
    }

    fn result(left: &[&str], right: &[&str], output: Output) -> Vec<String> {
        let left = strings(left);
        let right = strings(right);
        // SAFETY: the input and output buffers independently root all their strings.
        unsafe {
            let rows = materialize_words(&left, &right, output);
            Line::from_words(&rows)
                .iter()
                .map(|line| line.text().to_owned())
                .collect()
        }
    }

    fn check_observations(left: &[&str], right: &[&str]) {
        let full = result(left, right, Output::Full);
        let projected = result(left, right, Output::ProjectedChanges);
        for sign in ["+ ", "- "] {
            assert!(
                full.iter()
                    .filter(|s| s.starts_with(sign))
                    .eq(projected.iter().filter(|s| s.starts_with(sign))),
                "{left:?}, {right:?}"
            );
        }
        let sorted = |rows: Vec<String>| {
            let mut rows: Vec<_> = rows
                .into_iter()
                .filter(|s| s.starts_with(['+', '-']))
                .collect();
            rows.sort_by(|a, b| a[2..].cmp(&b[2..]));
            rows
        };
        assert_eq!(sorted(full), sorted(projected), "{left:?}, {right:?}");
    }

    #[test]
    fn raw_keeps_guides_and_interleaving() {
        let left = ["anchor", "same text abc", "same text def", "zeta"];
        let right = ["anchor", "same text abX", "same text deX", "zeta"];
        assert_eq!(
            result(&left, &right, Output::Full),
            [
                "  anchor",
                "- same text abc",
                "?             ^\n",
                "+ same text abX",
                "?             ^\n",
                "- same text def",
                "?             ^\n",
                "+ same text deX",
                "?             ^\n",
                "  zeta"
            ]
        );
        assert_eq!(
            result(&left, &right, Output::ProjectedChanges),
            [
                "- same text abc",
                "- same text def",
                "+ same text abX",
                "+ same text deX"
            ]
        );
    }

    #[test]
    fn projected_changes_preserve_each_side_and_sorted_ties() {
        // All orderings, multiplicities and near matches through length three.
        let mut sequences = vec![Vec::new()];
        for length in 1..=3 {
            for sequence in 0..3_usize.pow(length) {
                sequences.push(
                    (0..length)
                        .map(|i| ["café a", "café b", ""][sequence / 3_usize.pow(i) % 3])
                        .collect(),
                );
            }
        }
        for left in &sequences {
            for right in &sequences {
                check_observations(left, right);
            }
        }
        // The shared popular line cannot anchor a SequenceMatcher block, but
        // fancy replacement may emit it as unchanged. Keep that decision exact.
        let tail: Vec<_> = (0..200).map(|i| format!("tail {i:03}")).collect();
        let mut left = vec!["a", "same", "same", "same", "same", "u"];
        let mut right = vec!["b", "same", "same", "same", "same", "v"];
        left.extend(tail.iter().map(String::as_str));
        right.extend(tail.iter().map(String::as_str));
        check_observations(&left, &right);
        assert_eq!(
            result(&left, &right, Output::ProjectedChanges),
            ["- a", "+ b", "- u", "+ v"]
        );
    }
}
