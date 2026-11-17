//! Group exact Unicode scalars and their positions in a single flat word array.

use super::{Context, Id, Result, Value};

/// Conservatively ensure that every Unicode scalar and position fit in one word.
pub(super) fn can_pack(text: &str) -> bool {
    text.len() as u64 <= 1u64 << 43
}

struct Index {
    words: Vec<u64>,
    starts: Vec<usize>,
    order: Vec<usize>,
    bits: u32,
    mask: u64,
    len: usize,
}

impl Index {
    fn new(text: &str) -> Result<Self> {
        let len = text.chars().count();
        let bits = (u64::BITS - (len.saturating_sub(1) as u64).leading_zeros()).max(1);
        debug_assert!(bits <= 43);
        let mask = (1u64 << bits) - 1;
        let mut words = Vec::new();
        words
            .try_reserve_exact(len)
            .map_err(|_| crate::Error::memory_error())?;
        words.extend(
            text.chars()
                .enumerate()
                .map(|(i, scalar)| ((scalar as u64) << bits) | i as u64),
        );
        words.sort_unstable();

        let mut starts = Vec::new();
        for i in 0..words.len() {
            if i == 0 || words[i] >> bits != words[i - 1] >> bits {
                if starts.len() == starts.capacity() {
                    starts
                        .try_reserve(1)
                        .map_err(|_| crate::Error::memory_error())?;
                }
                starts.push(i);
            }
        }
        starts
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error())?;
        starts.push(words.len());
        let mut order = Vec::new();
        order
            .try_reserve_exact(starts.len() - 1)
            .map_err(|_| crate::Error::memory_error())?;
        order.extend(0..starts.len() - 1);
        order.sort_unstable_by_key(|&i| words[starts[i]] & mask);
        Ok(Self {
            words,
            starts,
            order,
            bits,
            mask,
            len,
        })
    }

    fn group(&self, i: usize) -> (char, &[u64]) {
        let words = &self.words[self.starts[i]..self.starts[i + 1]];
        let scalar = char::from_u32((words[0] >> self.bits) as u32).unwrap();
        (scalar, words)
    }
}

/// Materialize the public Unicode index after the caller excludes all source observers.
pub(super) fn build(ctx: &mut Context, receiver: Id, b: Id, autojunk: bool) -> Result<()> {
    let Value::Str(text) = ctx.heap.get(b) else {
        unreachable!()
    };
    debug_assert!(can_pack(text));
    let index = Index::new(text)?;
    let threshold = if autojunk && index.len >= 200 {
        index.len / 100 + 1
    } else {
        usize::MAX
    };

    ctx.heap.reserve_temporaries(4)?;
    let b2j = ctx.dict(Vec::new());
    ctx.heap.keep(b2j);
    ctx.default_set_attr(receiver, "b2j", b2j)?;
    for &group in &index.order {
        let (scalar, words) = index.group(group);
        if words.len() > threshold {
            continue;
        }
        let mark = ctx.heap.temporary_mark();
        let key = ctx.text(scalar.to_string());
        ctx.heap.keep(key);
        let mut values = Vec::new();
        values
            .try_reserve_exact(words.len())
            .map_err(|_| crate::Error::memory_error())?;
        let list = ctx.alloc(Value::List(values));
        ctx.set_dictionary_key(b2j, key, list, None, false)?;
        ctx.heap.try_extend_position_list(
            list,
            words.iter().map(|word| (word & index.mask) as usize),
        )?;
        ctx.heap.release_temporaries_since(mark);
    }
    let bjunk = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    ctx.heap.keep(bjunk);
    ctx.default_set_attr(receiver, "bjunk", bjunk)?;
    let popular = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    ctx.heap.keep(popular);
    ctx.default_set_attr(receiver, "bpopular", popular)?;
    for &group in &index.order {
        let (scalar, words) = index.group(group);
        if words.len() <= threshold {
            continue;
        }
        let mark = ctx.heap.temporary_mark();
        let key = ctx.text(scalar.to_string());
        ctx.heap.keep(key);
        super::super::super::builtins::set_add(ctx, popular, key)?;
        ctx.heap.release_temporaries_since(mark);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Index;

    #[test]
    fn grouped_scalar_words_restore_first_occurrence_order_and_positions() {
        let index = Index::new("雪Ω雪🧪Ωé").unwrap();
        assert_eq!(index.len, 6);
        let actual: Vec<_> = index
            .order
            .iter()
            .map(|&group| {
                let (scalar, words) = index.group(group);
                (
                    scalar,
                    words.iter().map(|w| w & index.mask).collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            actual,
            [
                ('雪', vec![0, 2]),
                ('Ω', vec![1, 4]),
                ('🧪', vec![3]),
                ('é', vec![5])
            ]
        );
    }
}
