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
    unsafe {
        let index = usize::from((*slot).index);
        (slot.sub(index).cast::<Block>(), index)
    }
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
    let (block, index) = unsafe {
        let (block, index) = locate(n.0);
        (&*block, index)
    };
    let offset = (index + 1) * block.width;
    &block.words[offset..offset + block.width]
}

pub(super) fn number(n: &Int) -> BigInt {
    from_words(row(n))
}

fn from_words(words: &[u64]) -> BigInt {
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

pub(super) fn bits(n: &Int) -> u64 {
    let words = row(n);
    let mask = sign_extension(*words.last().unwrap());
    let mut carry = u64::from(mask != 0);
    let mut bits = 0;
    for (i, &word) in words.iter().enumerate() {
        let (magnitude, overflow) = (word ^ mask).overflowing_add(carry);
        carry = u64::from(overflow);
        if magnitude != 0 {
            bits = i as u64 * 64 + u64::from(64 - magnitude.leading_zeros());
        }
    }
    bits
}

fn sign_extension(low: u64) -> u64 {
    ((low as i64) >> 63) as u64
}

/// Extend the operand to the row width. Negative magnitudes use complement plus one.
#[inline]
fn add_words(result: &mut [u64], mut digits: impl Iterator<Item = u64>, fill: u64, flip: u64) {
    let mut carry = u64::from(flip != 0);
    for out in result {
        let right = digits.next().unwrap_or(fill) ^ flip;
        let sum = u128::from(*out) + u128::from(right) + u128::from(carry);
        *out = sum as u64;
        carry = (sum >> 64) as u64;
    }
}

fn add(result: &mut [u64], item: &Int) {
    if let Some(n) = item.as_small() {
        let word = n as u64;
        add_words(result, std::iter::once(word), sign_extension(word), 0);
    } else if item.0 & TAG != 0 {
        let words = row(item);
        add_words(
            result,
            words.iter().copied(),
            sign_extension(*words.last().unwrap()),
            0,
        );
    } else {
        let n = item.big();
        let flip = if n.sign() == Sign::Minus { u64::MAX } else { 0 };
        add_words(result, n.iter_u64_digits(), 0, flip);
    }
}

pub(super) fn extend(output: &mut Vec<Int>, items: &[Int]) -> Result<()> {
    for group in items.chunks(SLOTS) {
        let previous = output.last().unwrap();
        let largest = group.iter().map(Int::bits).fold(previous.bits(), u64::max);
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
        add(&mut words[..width], previous);
        for (index, item) in group.iter().enumerate() {
            let (before, after) = words.split_at_mut((index + 1) * width);
            let result = &mut after[..width];
            result.copy_from_slice(&before[index * width..]);
            add(result, item);
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

/// Reduce materialized prefixes directly from their signed rows.
pub(super) fn sum<'a>(items: impl Iterator<Item = &'a Int> + Clone) -> Option<Int> {
    let upper = items.size_hint().1?;
    let largest = items.clone().map(Int::bits).max()?;
    let carry_bits = usize::BITS - upper.saturating_sub(1).leading_zeros();
    let width = largest
        .saturating_add(u64::from(carry_bits) + 1)
        .div_ceil(64) as usize;
    if width > MAX_WORDS {
        return None;
    }
    let mut result = vec![0; width];
    for item in items {
        add(&mut result, item);
    }
    Some(Int::new(from_words(&result)))
}

/// Write a short packed integer directly from its signed row. Groups of 19
/// decimal digits fit in one word, and the leading 128 bits use integer formatting.
pub(super) fn write_short_decimal(
    n: &Int,
    output: &mut impl std::fmt::Write,
    check_digits: bool,
) -> Option<std::fmt::Result> {
    let words = row(n);
    if words.len() > 4 {
        return None;
    }
    let mask = sign_extension(*words.last().unwrap());
    let mut magnitude = [0_u64; 4];
    let mut carry = u64::from(mask != 0);
    for (slot, &word) in magnitude.iter_mut().zip(words) {
        let (digit, overflow) = (word ^ mask).overflowing_add(carry);
        *slot = digit;
        carry = u64::from(overflow);
    }
    let mut length = magnitude.iter().rposition(|&v| v != 0).map_or(0, |i| i + 1);
    const BASE: u128 = 10_000_000_000_000_000_000;
    let mut tails = [0_u64; 3];
    let mut count = 0;
    while length > 2 {
        let mut remainder = 0_u128;
        for digit in magnitude[..length].iter_mut().rev() {
            let current = (remainder << 64) | u128::from(*digit);
            *digit = (current / BASE) as u64;
            remainder = current % BASE;
        }
        tails[count] = remainder as u64;
        count += 1;
        while length > 0 && magnitude[length - 1] == 0 {
            length -= 1;
        }
    }
    let head = (u128::from(magnitude[1]) << 64) | u128::from(magnitude[0]);
    let mut buffer = itoa::Buffer::new();
    let leading = buffer.format(head);
    if check_digits && !crate::integer_text::decimal_digits_allowed(leading.len() + count * 19) {
        return None;
    }
    Some((|| {
        if mask != 0 {
            output.write_char('-')?;
        }
        output.write_str(leading)?;
        let mut buffer = itoa::Buffer::new();
        for &tail in tails[..count].iter().rev() {
            let text = buffer.format(tail);
            output.write_str(&"0000000000000000000"[text.len()..])?;
            output.write_str(text)?;
        }
        Ok(())
    })())
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
