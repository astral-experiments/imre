//! Compute matching blocks from one authoritative flat index without private caches.

use super::{AsciiPositions, Context, Id, Result, Value, matcher};

fn push<T>(items: &mut Vec<T>, item: T) -> Result<()> {
    if items.len() == items.capacity() {
        items
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error())?;
    }
    items.push(item);
    Ok(())
}

fn search(left: &[u8], right: &[u8], index: &AsciiPositions) -> Result<Vec<[usize; 3]>> {
    let mut queue = Vec::new();
    push(&mut queue, [0, left.len(), 0, right.len()])?;
    let mut matches = Vec::new();
    while let Some([alo, ahi, blo, bhi]) = queue.pop() {
        let (i, j, size) = matcher::longest_ascii(
            left,
            right,
            (alo, ahi, blo, bhi),
            &index.offsets,
            &index.positions,
            &index.junk,
        )?;
        if size == 0 {
            continue;
        }
        push(&mut matches, [i, j, size])?;
        if alo < i && blo < j {
            push(&mut queue, [alo, i, blo, j])?;
        }
        if i + size < ahi && j + size < bhi {
            push(&mut queue, [i + size, ahi, j + size, bhi])?;
        }
    }
    matches.sort_unstable();
    let mut result = Vec::new();
    result
        .try_reserve_exact(matches.len().saturating_add(1))
        .map_err(|_| crate::Error::memory_error())?;
    let mut previous = [0, 0, 0];
    for [i, j, size] in matches {
        if previous[0] + previous[2] == i && previous[1] + previous[2] == j {
            previous[2] += size;
        } else {
            if previous[2] != 0 {
                result.push(previous);
            }
            previous = [i, j, size];
        }
    }
    if previous[2] != 0 {
        result.push(previous);
    }
    result.push([left.len(), right.len(), 0]);
    Ok(result)
}

/// Build the source-visible list and its fresh Match values after complete callback preflight.
/// Return no admission if a signal arrived before publishing the list to the receiver.
pub(super) fn build(
    ctx: &mut Context,
    receiver: Id,
    a: Id,
    b: Id,
    index: &AsciiPositions,
    match_class: Id,
) -> Result<Option<Id>> {
    let blocks = {
        let (Value::Str(a), Value::Str(b)) = (ctx.heap.get(a), ctx.heap.get(b)) else {
            unreachable!()
        };
        debug_assert!(a.is_ascii() && b.is_ascii());
        search(a.as_bytes(), b.as_bytes(), index)?
    };
    let mut values = Vec::new();
    values
        .try_reserve_exact(blocks.len())
        .map_err(|_| crate::Error::memory_error())?;
    let mark = ctx.heap.temporary_mark();
    ctx.heap
        .reserve_temporaries(blocks.len().saturating_add(5))?;
    for [i, j, size] in blocks {
        let block_mark = ctx.heap.temporary_mark();
        let a = ctx.int(i as i64);
        ctx.heap.keep(a);
        let b = ctx.int(j as i64);
        ctx.heap.keep(b);
        let size = ctx.int(size as i64);
        ctx.heap.keep(size);
        let storage = ctx.alloc(Value::Tuple(vec![a, b, size]));
        ctx.heap.keep(storage);
        let value = ctx.try_plain_instance(match_class)?;
        let Value::Instance { scalar, .. } = ctx.heap.get_mut(value) else {
            unreachable!()
        };
        *scalar = Some(storage);
        ctx.heap.release_temporaries_since(block_mark);
        ctx.heap.keep(value);
        values.push(value);
    }
    let list = ctx.alloc(Value::List(values));
    ctx.heap.keep(list);
    #[cfg(unix)]
    if crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Acquire) {
        ctx.heap.release_temporaries_since(mark);
        return Ok(None);
    }
    ctx.default_set_attr(receiver, "matching_blocks", list)?;
    ctx.heap.release_temporaries_since(mark);
    Ok(Some(list))
}

#[cfg(test)]
mod tests {
    use super::{AsciiPositions, search};

    fn index(right: &[u8]) -> AsciiPositions {
        let mut offsets = [0; 257];
        let mut positions = Vec::new();
        for byte in 0..256 {
            positions.extend(
                right
                    .iter()
                    .enumerate()
                    .filter_map(|(i, value)| (*value as usize == byte).then_some(i)),
            );
            offsets[byte + 1] = positions.len();
        }
        AsciiPositions {
            offsets,
            positions,
            junk: [false; 256],
        }
    }

    #[test]
    fn blocks_are_sorted_collapsed_and_terminated() {
        let right = b"abycd";
        assert_eq!(
            search(b"abxcd", right, &index(right)).unwrap(),
            [[0, 0, 2], [3, 3, 2], [5, 5, 0]]
        );
        assert_eq!(search(b"", b"", &index(b"")).unwrap(), [[0, 0, 0]]);
    }
}
