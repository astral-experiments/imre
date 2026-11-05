//! Each wide integer owns a slot in an immutable block of signed prefix rows.

use super::{BigInt, Error, Int, Result, Shared};
use num_bigint::Sign;

// The three low union and extension tags stay available on every integer word.
pub(super) const TAG: usize = 8;
const SLOTS: usize = 32;
const MAX_WORDS: usize = 64;

#[repr(align(16))]
struct Slot {
    index: u8,
}

// A slot's index recovers this block without a pointer per result.
#[repr(C)]
struct Block {
    slots: [Slot; SLOTS],
    width: usize,
    words: Vec<u64>,
}

/// The caller borrows or consumes a live reference to this packed integer.
unsafe fn locate(word: usize) -> (*const Block, usize) {
    let slot = (word & !TAG) as *const Slot;
    // SAFETY: the supplied slot belongs to the first field of its owning block.
    let index = usize::from(unsafe { (*slot).index });
    (unsafe { slot.sub(index).cast::<Block>() }, index)
}

pub(super) unsafe fn retain(word: usize) {
    // SAFETY: the supplied word roots its block.
    unsafe { Shared::increment_strong_count(locate(word).0) };
}

pub(super) unsafe fn release(word: usize) {
    // SAFETY: the supplied word transfers one strong reference to its block.
    unsafe { Shared::decrement_strong_count(locate(word).0) };
}

fn row(n: &Int) -> &[u64] {
    assert_eq!(n.0 & (TAG | 1), TAG);
    // SAFETY: the integer owns its immutable block for this entire borrow.
    let (block, index) = unsafe { locate(n.0) };
    let block = unsafe { &*block };
    let offset = (index + 1) * block.width;
    &block.words[offset..offset + block.width]
}

pub(super) fn number(n: &Int) -> BigInt {
    let words = row(n);
    #[cfg(target_endian = "little")]
    {
        // SAFETY: all bytes of these initialized u64s are valid and immutable.
        let bytes =
            unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), size_of_val(words)) };
        BigInt::from_signed_bytes_le(bytes)
    }
    #[cfg(target_endian = "big")]
    {
        BigInt::from_signed_bytes_le(
            &words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
    }
}

fn bits(n: &Int) -> u64 {
    if let Some(n) = n.as_small() {
        return u64::from(u64::BITS - n.unsigned_abs().leading_zeros());
    }
    if n.0 & TAG == 0 {
        return n.big().bits();
    }
    let words = row(n);
    let mask = sign_extension(*words.last().unwrap());
    let significant = words.iter().rposition(|&word| word != mask);
    significant.map_or(0, |i| {
        64 * i as u64 + u64::from(64 - (words[i] ^ mask).leading_zeros())
    }) + u64::from(mask != 0)
}

fn sign_extension(low: u64) -> u64 {
    ((low as i64) >> 63) as u64
}

/// Extend the operand to the row width. Negative magnitudes use complement plus one.
#[inline]
fn add_words(
    result: &mut [u64],
    previous: &[u64],
    mut digits: impl Iterator<Item = u64>,
    fill: u64,
    flip: u64,
) {
    let mut carry = u64::from(flip != 0);
    for (out, &left) in result.iter_mut().zip(previous) {
        let right = digits.next().unwrap_or(fill) ^ flip;
        let sum = u128::from(left) + u128::from(right) + u128::from(carry);
        *out = sum as u64;
        carry = (sum >> 64) as u64;
    }
}

fn write_sum(result: &mut [u64], previous: &[u64], item: &Int) {
    if let Some(n) = item.as_small() {
        let word = n as u64;
        add_words(
            result,
            previous,
            std::iter::once(word),
            sign_extension(word),
            0,
        );
    } else if item.0 & TAG != 0 {
        let words = row(item);
        add_words(
            result,
            previous,
            words.iter().copied(),
            sign_extension(*words.last().unwrap()),
            0,
        );
    } else {
        let n = item.big();
        let flip = if n.sign() == Sign::Minus { u64::MAX } else { 0 };
        add_words(result, previous, n.iter_u64_digits(), 0, flip);
    }
}

pub(super) fn extend(output: &mut Vec<Int>, items: &[Int]) -> Result<()> {
    for group in items.chunks(SLOTS) {
        let previous = output.last().unwrap();
        let largest = group.iter().map(bits).fold(bits(previous), u64::max);
        // The starting value and at most 32 addends need six carry bits and a sign.
        let width = largest.saturating_add(7).div_ceil(64) as usize;
        if width > MAX_WORDS {
            for item in group {
                output.push(output.last().unwrap().add(item));
            }
            continue;
        }
        let mut words = Vec::new();
        let count = (group.len() + 1) * width;
        words
            .try_reserve_exact(count)
            .map_err(|_| Error::allocation())?;
        words.resize(count, 0);
        const ZERO: [u64; MAX_WORDS] = [0; MAX_WORDS];
        write_sum(&mut words[..width], &ZERO[..width], previous);
        for (index, item) in group.iter().enumerate() {
            let (before, after) = words.split_at_mut((index + 1) * width);
            write_sum(&mut after[..width], &before[index * width..], item);
        }
        let block = Shared::try_new(Block {
            slots: std::array::from_fn(|i| Slot { index: i as u8 }),
            width,
            words,
        })
        .map_err(|_| Error::allocation())?;
        for (slot, digits) in block
            .slots
            .iter()
            .zip(block.words[width..].chunks_exact(width))
        {
            let low = digits[0];
            if let Some(small) = Int::small(low as i64)
                && digits[1..].iter().all(|&word| word == sign_extension(low))
            {
                output.push(small);
            } else {
                std::mem::forget(Shared::clone(&block));
                output.push(Int(std::ptr::from_ref(slot) as usize | TAG));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_traits::One;

    #[test]
    fn wide_prefixes_outlive_their_list() {
        let seed: BigInt = BigInt::one() << 110;
        let huge: BigInt = BigInt::one() << 5000;
        let operands: Vec<BigInt> = (0..99)
            .map(|i| match i {
                33 => huge.clone(),
                65 => -&huge,
                _ => BigInt::from(i - 48),
            })
            .collect();
        let mut expected = vec![seed.clone()];
        for operand in &operands {
            expected.push(expected.last().unwrap() + operand);
        }
        let items: Vec<_> = operands.into_iter().map(Int::new).collect();
        let output = crate::integer_prefixes(&items, Some(&Int::new(seed)))
            .ok()
            .unwrap();
        assert!(
            output
                .iter()
                .map(|n| n.big().into_owned())
                .eq(expected.iter().cloned())
        );
        let survivors: Vec<_> = [2, 32, 33, 34, 65, 66, 97, 99]
            .map(|i| (i, output[i].clone()))
            .into();
        assert!(survivors.iter().any(|(_, n)| n.0 & TAG != 0));
        drop(output);
        drop(items);
        for (i, n) in survivors {
            assert_eq!(n.big().as_ref(), &expected[i]);
        }
    }
}
