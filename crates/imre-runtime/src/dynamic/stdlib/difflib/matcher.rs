//! Longest-match search over the caller's current public `b2j` positions.

use super::super::{Error, Result};

fn zeroed_words(length: usize) -> Result<Vec<usize>> {
    let mut words = Vec::new();
    words
        .try_reserve_exact(length)
        .map_err(|_| Error::new("MemoryError", ""))?;
    words.resize(length, 0);
    Ok(words)
}

/// Find CPython's longest contiguous match using the live, ordered `b2j` lists.
///
/// `offsets` partitions `positions` by byte. Each partition retains its Python
/// list order: a caller can reorder, duplicate, or edit positions, and CPython
/// stops at the first entry at or above `bhi`. The two dense rows keep previous
/// and current results separate even when those lists are not sorted.
pub(super) fn longest_ascii(
    left: &[u8],
    right: &[u8],
    bounds: (usize, usize, usize, usize),
    offsets: &[usize; 257],
    positions: &[usize],
    junk: &[bool; 256],
) -> Result<(usize, usize, usize)> {
    longest_indexed(
        left,
        right,
        bounds,
        !positions.is_empty(),
        |byte| &positions[offsets[byte as usize]..offsets[byte as usize + 1]],
        |byte| junk[byte as usize],
    )
}

/// Search scalar-indexed Unicode strings through sorted live public symbol partitions.
pub(super) fn longest_unicode(
    left: &[char],
    right: &[char],
    bounds: (usize, usize, usize, usize),
    keys: &[char],
    offsets: &[usize],
    positions: &[usize],
    junk: &[char],
) -> Result<(usize, usize, usize)> {
    longest_indexed(
        left,
        right,
        bounds,
        !positions.is_empty(),
        |symbol| match keys.binary_search(&symbol) {
            Ok(i) => &positions[offsets[i]..offsets[i + 1]],
            Err(_) => &[],
        },
        |symbol| junk.binary_search(&symbol).is_ok(),
    )
}

fn longest_indexed<'p, T: Copy + Eq>(
    left: &[T],
    right: &[T],
    bounds: (usize, usize, usize, usize),
    has_positions: bool,
    hits: impl Fn(T) -> &'p [usize],
    is_junk: impl Fn(T) -> bool,
) -> Result<(usize, usize, usize)> {
    let (alo, ahi, blo, bhi) = bounds;
    debug_assert!(alo <= ahi && ahi <= left.len() && blo <= bhi && bhi <= right.len());
    let width = bhi - blo;
    let maximum = (ahi - alo).min(width);
    let mut best = (alo, blo, 0);
    if maximum != 0 && has_positions {
        let mut previous = zeroed_words(width)?;
        let mut current = zeroed_words(width)?;
        let length_bits = usize::BITS - maximum.leading_zeros();
        let packed = length_bits < usize::BITS
            && ahi - alo <= usize::MAX.checked_shr(length_bits).unwrap_or(0);

        if packed {
            // Pack the row number above the match length. This avoids clearing
            // a complete row for every character while using only two arrays.
            let length_mask = (1usize << length_bits) - 1;
            for (row, &byte) in left[alo..ahi].iter().enumerate() {
                let epoch = row + 1;
                for &j in hits(byte) {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let word = if j > blo { previous[j - blo - 1] } else { 0 };
                    let prior = if word >> length_bits == epoch - 1 {
                        word & length_mask
                    } else {
                        0
                    };
                    let length = prior + 1;
                    current[j - blo] = (epoch << length_bits) | length;
                    if length > best.2 {
                        best = (alo + row + 1 - length, j + 1 - length, length);
                    }
                }
                std::mem::swap(&mut previous, &mut current);
            }
        } else {
            // Extremely large bounds may not leave enough bits for the row
            // number. Keep the same two arrays and clear only in this case.
            for (row, &byte) in left[alo..ahi].iter().enumerate() {
                current.fill(0);
                for &j in hits(byte) {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let length = if j > blo {
                        previous[j - blo - 1] + 1
                    } else {
                        1
                    };
                    current[j - blo] = length;
                    if length > best.2 {
                        best = (alo + row + 1 - length, j + 1 - length, length);
                    }
                }
                std::mem::swap(&mut previous, &mut current);
            }
        }
    }

    // Popular items may extend a match but cannot anchor it. Junk extends
    // only after the non-junk extension has completed in both directions.
    for extend_junk in [false, true] {
        while best.0 > alo
            && best.1 > blo
            && is_junk(right[best.1 - 1]) == extend_junk
            && left[best.0 - 1] == right[best.1 - 1]
        {
            best.0 -= 1;
            best.1 -= 1;
            best.2 += 1;
        }
        while best.0 + best.2 < ahi
            && best.1 + best.2 < bhi
            && is_junk(right[best.1 + best.2]) == extend_junk
            && left[best.0 + best.2] == right[best.1 + best.2]
        {
            best.2 += 1;
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::{longest_ascii, longest_unicode};

    fn one_list(byte: u8, list: &[usize]) -> ([usize; 257], Vec<usize>) {
        let mut offsets = [0; 257];
        offsets[byte as usize + 1..].fill(list.len());
        (offsets, list.to_vec())
    }

    #[test]
    fn edited_position_lists_retain_order_duplicates_and_breaks() {
        let (offsets, positions) = one_list(b'x', &[2, 0]);
        let no_junk = [false; 256];
        assert_eq!(
            longest_ascii(b"x", b"abc", (0, 1, 0, 2), &offsets, &positions, &no_junk).unwrap(),
            (0, 0, 0)
        );
        assert_eq!(
            longest_ascii(b"x", b"abc", (0, 1, 0, 3), &offsets, &positions, &no_junk).unwrap(),
            (0, 2, 1)
        );
        let (offsets, positions) = one_list(b'x', &[0, 1, 1, 0, 2]);
        assert_eq!(
            longest_ascii(b"xx", b"ABC", (0, 2, 0, 3), &offsets, &positions, &no_junk).unwrap(),
            (0, 0, 2)
        );
    }

    #[test]
    fn live_junk_stops_non_junk_extension_before_junk_extension() {
        let (offsets, positions) = one_list(b'x', &[0]);
        let mut junk = [false; 256];
        junk[b'y' as usize] = true;
        assert_eq!(
            longest_ascii(b"xyZ", b"xyZ", (0, 3, 0, 3), &offsets, &positions, &junk).unwrap(),
            (0, 0, 2)
        );
    }

    #[test]
    fn unicode_uses_scalar_offsets_and_live_junk() {
        let left: Vec<_> = "ΩéZ".chars().collect();
        let right = left.clone();
        assert_eq!(
            longest_unicode(&left, &right, (0, 3, 0, 3), &['Ω'], &[0, 1], &[0], &['é']).unwrap(),
            (0, 0, 2)
        );
    }
}
