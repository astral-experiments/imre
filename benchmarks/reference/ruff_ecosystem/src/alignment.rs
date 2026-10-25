use std::collections::HashMap;
use std::hash::Hash;

/// The reporter drops unchanged rows and intraline hints, then stably sorts by row text.
pub fn changes<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(bool, &'a str)> {
    let mut edits = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        match old[i].cmp(new[j]) {
            std::cmp::Ordering::Less => {
                edits.push((false, old[i]));
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                edits.push((true, new[j]));
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                let row = old[i];
                let start = j;
                while i < old.len() && j < new.len() && old[i] == row && new[j] == row {
                    i += 1;
                    j += 1;
                }
                // Equal-length, nonpopular runs anchor in SequenceMatcher in sorted order.
                // Other shared runs need its exact tie-breaking and junk suppression.
                if old.get(i) == Some(&row)
                    || new.get(j) == Some(&row)
                    || (new.len() >= 200 && j - start > new.len() / 100 + 1)
                {
                    return aligned_changes(old, new);
                }
            }
        }
    }
    edits.extend(old[i..].iter().map(|s| (false, *s)));
    edits.extend(new[j..].iter().map(|s| (true, *s)));
    edits
}

fn matching_blocks<T: Eq + Hash>(
    a: &[T],
    b: &[T],
    junk: impl Fn(&T) -> bool,
) -> Vec<(usize, usize, usize)> {
    let mut positions: HashMap<&T, Vec<usize>> = HashMap::new();
    for (j, item) in b.iter().enumerate() {
        if !junk(item) {
            positions.entry(item).or_default().push(j);
        }
    }
    if b.len() >= 200 {
        positions.retain(|_, js| js.len() <= b.len() / 100 + 1);
    }
    let mut regions = vec![(0, a.len(), 0, b.len())];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = regions.pop() {
        let (mut ai, mut bj, mut size) = (alo, blo, 0);
        let mut prev = HashMap::new();
        for (i, item) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut next = HashMap::new();
            if let Some(js) = positions.get(item) {
                for &j in &js[js.partition_point(|&j| j < blo)..] {
                    if j >= bhi {
                        break;
                    }
                    let k = 1 + prev.get(&j).copied().unwrap_or(0);
                    next.insert(j + 1, k);
                    if k > size {
                        (ai, bj, size) = (i + 1 - k, j + 1 - k, k);
                    }
                }
            }
            prev = next;
        }
        for include_junk in [false, true] {
            while ai > alo && bj > blo && junk(&b[bj - 1]) == include_junk && a[ai - 1] == b[bj - 1]
            {
                ai -= 1;
                bj -= 1;
                size += 1;
            }
            while ai + size < ahi
                && bj + size < bhi
                && junk(&b[bj + size]) == include_junk
                && a[ai + size] == b[bj + size]
            {
                size += 1;
            }
        }
        if size != 0 {
            blocks.push((ai, bj, size));
            if alo < ai && blo < bj {
                regions.push((alo, ai, blo, bj));
            }
            if ai + size < ahi && bj + size < bhi {
                regions.push((ai + size, ahi, bj + size, bhi));
            }
        }
    }
    blocks.sort_unstable();
    blocks.push((a.len(), b.len(), 0));
    blocks
}

fn aligned_changes<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(bool, &'a str)> {
    let mut edits = Vec::new();
    let (mut alo, mut blo) = (0, 0);
    for (ai, bi, size) in matching_blocks(a, b, |_| false) {
        replacement(&a[alo..ai], &b[blo..bi], &mut edits);
        (alo, blo) = (ai + size, bi + size);
    }
    edits.sort_by_key(|&(_, row)| row);
    edits
}

/// Order a replace block like CPython 3.12's Differ, retaining only the +/- rows.
fn replacement<'a>(a: &[&'a str], b: &[&'a str], edits: &mut Vec<(bool, &'a str)>) {
    let mut work = vec![(a, b)];
    while let Some((old, new)) = work.pop() {
        let mut identical = None;
        let mut similar = None;
        let mut best = 0.74;
        for (j, &right) in new.iter().enumerate() {
            for (i, &left) in old.iter().enumerate() {
                if left == right {
                    identical.get_or_insert((i, j));
                    continue;
                }
                let x: Vec<char> = left.chars().collect();
                let y: Vec<char> = right.chars().collect();
                if (2 * x.len().min(y.len())) as f64 <= best * (x.len() + y.len()) as f64 {
                    continue;
                }
                let n: usize = matching_blocks(&x, &y, |c| matches!(c, ' ' | '\t'))
                    .iter()
                    .map(|m| m.2)
                    .sum();
                let ratio = (2 * n) as f64 / (x.len() + y.len()) as f64;
                if ratio > best {
                    best = ratio;
                    similar = Some((i, j));
                }
            }
        }
        let anchor = if best >= 0.75 { similar } else { identical };
        if let Some((i, j)) = anchor {
            work.push((&old[i + 1..], &new[j + 1..]));
            if old[i] != new[j] {
                // Right first on our stack so the stable sort sees '-' before '+'.
                work.push((&old[0..0], &new[j..=j]));
                work.push((&old[i..=i], &new[0..0]));
            }
            work.push((&old[..i], &new[..j]));
        } else {
            let minus = old.iter().map(|row| (false, *row));
            let plus = new.iter().map(|row| (true, *row));
            if new.len() < old.len() {
                edits.extend(plus);
                edits.extend(minus);
            } else {
                edits.extend(minus);
                edits.extend(plus);
            }
        }
    }
}
