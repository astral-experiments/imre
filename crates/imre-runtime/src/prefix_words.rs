//! Immutable integer prefixes share a bounded block of magnitude digits. Each
//! returned word owns the block containing its slot, independently of the list.

use super::{BigInt, Error, Int, Result, Shared};
use num_bigint::Sign;
use num_traits::ToPrimitive;

// Keep the low three union and extension tags available on every integer handle.
pub(super) const TAG: usize = 8;
const SLOTS: usize = 32;
const MAX_BITS: u64 = 4096;

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Slot {
    offset: u16,
    length: i16,
    index: u8,
}

// A slot's index recovers this block without a pointer per result.
#[repr(C)]
struct Block {
    slots: [Slot; SLOTS],
    digits: Vec<u32>,
}

const _: () = assert!(SLOTS * (MAX_BITS as usize / 32) <= u16::MAX as usize);

/// The caller borrows or consumes a live reference to this packed integer.
unsafe fn locate(word: usize) -> (*const Block, *const Slot) {
    let slot = (word & !TAG) as *const Slot;
    // SAFETY: every packed word points into the first field of its owning block.
    let block = unsafe { slot.sub((*slot).index as usize).cast::<Block>() };
    (block, slot)
}

pub(super) unsafe fn retain(word: usize) {
    // SAFETY: the supplied word roots its block.
    unsafe { Shared::increment_strong_count(locate(word).0) };
}

pub(super) unsafe fn release(word: usize) {
    // SAFETY: the supplied word transfers one strong reference to its block.
    unsafe { Shared::decrement_strong_count(locate(word).0) };
}

pub(super) unsafe fn number(word: usize) -> BigInt {
    // SAFETY: the owning integer keeps both the entry and the frozen digits live.
    let (block, slot) = unsafe { locate(word) };
    let (block, slot) = unsafe { (&*block, &*slot) };
    let start = usize::from(slot.offset);
    let end = start + usize::from(slot.length.unsigned_abs());
    let sign = if slot.length < 0 {
        Sign::Minus
    } else {
        Sign::Plus
    };
    BigInt::from_slice(sign, &block.digits[start..end])
}

enum Total {
    Fixed(i128),
    Big(BigInt),
}

impl Total {
    fn add(&mut self, item: &Int) {
        if let Self::Fixed(n) = self {
            let operand = item
                .as_small()
                .map(i128::from)
                .or_else(|| item.big().to_i128());
            if let Some(sum) = operand.and_then(|v| n.checked_add(v)) {
                *n = sum;
                return;
            }
            *self = Self::Big(BigInt::from(*n));
        }
        let Self::Big(n) = self else { unreachable!() };
        if let Some(v) = item.as_small() {
            *n += v;
        } else {
            *n += item.big().as_ref();
        }
    }

    fn small(&self) -> Option<Int> {
        match self {
            Self::Fixed(n) => i64::try_from(*n).ok(),
            Self::Big(n) => n.to_i64(),
        }
        .and_then(Int::small)
    }

    fn magnitude(&self) -> (u64, bool) {
        match self {
            Self::Fixed(n) => (
                u64::from(i128::BITS - n.unsigned_abs().leading_zeros()),
                *n < 0,
            ),
            Self::Big(n) => (n.bits(), n.sign() == Sign::Minus),
        }
    }

    fn append_digits(&self, digits: &mut Vec<u32>, length: usize) {
        match self {
            Self::Fixed(n) => {
                let mut magnitude = n.unsigned_abs();
                for _ in 0..length {
                    digits.push(magnitude as u32);
                    magnitude >>= 32;
                }
            }
            Self::Big(n) => digits.extend(n.iter_u32_digits()),
        }
    }
}

pub(super) fn extend(output: &mut Vec<Int>, items: &[Int]) -> Result<()> {
    let previous = output.last().unwrap().big();
    let mut total = previous
        .to_i128()
        .map_or_else(|| Total::Big(previous.into_owned()), Total::Fixed);
    for group in items.chunks(SLOTS) {
        let first = output.len();
        let mut block = None;
        for (index, item) in group.iter().enumerate() {
            total.add(item);
            if let Some(small) = total.small() {
                output.push(small);
                continue;
            }
            let (bits, negative) = total.magnitude();
            if bits > MAX_BITS {
                let Total::Big(n) = &total else {
                    unreachable!()
                };
                output.push(Int::new(n.clone()));
                continue;
            }
            let length = bits.div_ceil(32) as usize;
            if block.is_none() {
                let mut digits = Vec::new();
                digits
                    .try_reserve_exact(length * (group.len() - index))
                    .map_err(|_| Error::allocation())?;
                block = Some(
                    Shared::try_new(Block {
                        slots: [Slot::default(); SLOTS],
                        digits,
                    })
                    .map_err(|_| Error::allocation())?,
                );
            }
            let storage = Shared::get_mut(block.as_mut().unwrap()).unwrap();
            let offset = storage.digits.len() as u16;
            storage
                .digits
                .try_reserve(length)
                .map_err(|_| Error::allocation())?;
            total.append_digits(&mut storage.digits, length);
            let signed = if negative {
                -(length as i16)
            } else {
                length as i16
            };
            storage.slots[index] = Slot {
                offset,
                length: signed,
                index: index as u8,
            };
            output.push(Int::from(0));
        }
        if let Some(block) = block {
            for (index, slot) in block.slots[..group.len()].iter().enumerate() {
                if slot.length != 0 {
                    let word = std::ptr::from_ref(slot) as usize | TAG;
                    std::mem::forget(Shared::clone(&block));
                    output[first + index] = Int(word);
                }
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
        let output = crate::integer_prefixes(&items, Some(&Int::new(seed))).unwrap();
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
