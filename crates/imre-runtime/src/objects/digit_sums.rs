//! Allocation-free reductions over the decimal digits in an immutable string.

use crate::{Int, Str};

type Tables = [[u8; 16]; 2];

/// Returns packed 31-bit subtotals for the two right-aligned strides, or the
/// immediate -1 if the caller must execute its original conversion and folds.
///
/// # Safety
/// The caller roots the string throughout the call and provides two immutable
/// 16-byte tables. Entries zero through nine are the checked digit contributions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_try_digit_sums(text: *const Str, tables: *const Tables) -> usize {
    // SAFETY: both pointees remain immutable for the entire borrowed reduction.
    let (bytes, tables) = unsafe { ((&*text).as_bytes(), &*tables) };
    let packed = totals(bytes, tables).map_or(-1, |[a, b]| (b << 31 | a) as i64);
    Int::small(packed).unwrap().into_raw()
}

fn totals(mut bytes: &[u8], tables: &Tables) -> Option<[u64; 2]> {
    // Each lane sees at most ceil(n / 2) one-byte contributions. Both results
    // must fit one immediate even if every digit maps to 255.
    if bytes.len().div_ceil(2) > i32::MAX as usize / 255
        || bytes.first().is_some_and(|b| !b.is_ascii_digit())
    {
        return None;
    }
    let mut leading = 0;
    if !bytes.len().is_multiple_of(2) {
        leading = u64::from(tables[0][usize::from(bytes[0] - b'0')]);
        bytes = &bytes[1..];
    }
    let mut sum = [0; 2];
    #[cfg(target_arch = "x86_64")]
    if bytes.len() >= 16 && std::is_x86_feature_detected!("ssse3") {
        let (blocks, tail) = bytes.as_chunks::<16>();
        // SAFETY: the feature was detected and every block contains sixteen bytes.
        sum = unsafe { x86_blocks(blocks, tables) }?.map(u64::from);
        bytes = tail;
    }
    #[cfg(all(
        target_arch = "aarch64",
        target_endian = "little",
        target_feature = "neon"
    ))]
    if bytes.len() >= 16 {
        let (blocks, tail) = bytes.as_chunks::<16>();
        sum = arm_blocks(blocks, tables)?;
        bytes = tail;
    }
    sum[0] += leading;
    for &[a, b] in bytes.as_chunks::<2>().0 {
        let a = a.wrapping_sub(b'0');
        let b = b.wrapping_sub(b'0');
        if a.max(b) > 9 {
            return None;
        };
        sum[0] += u64::from(tables[0][usize::from(b)]);
        sum[1] += u64::from(tables[1][usize::from(a)]);
    }
    Some(sum)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn x86_blocks(blocks: &[[u8; 16]], tables: &Tables) -> Option<[u32; 2]> {
    use std::arch::x86_64::*;
    // SAFETY: each array spans exactly one unaligned SIMD load.
    let [right, left] = tables
        .each_ref()
        .map(|t| unsafe { _mm_loadu_si128(t.as_ptr().cast()) });
    let zero = _mm_setzero_si128();
    // Each eight-byte half contributes to one right-aligned stride.
    let parity = _mm_setr_epi8(1, 3, 5, 7, 9, 11, 13, 15, 0, 2, 4, 6, 8, 10, 12, 14);
    let mut sums = zero;
    for block in blocks {
        // SAFETY: as_chunks proved that all sixteen bytes belong to the string.
        let raw = unsafe { _mm_loadu_si128(block.as_ptr().cast()) };
        let d = _mm_sub_epi8(raw, _mm_set1_epi8(b'0' as i8));
        if _mm_movemask_epi8(_mm_cmpeq_epi8(_mm_subs_epu8(d, _mm_set1_epi8(9)), zero)) != 0xffff {
            return None;
        }
        let digits = _mm_shuffle_epi8(d, parity);
        let right_terms = _mm_shuffle_epi8(right, digits);
        let left_terms = _mm_shuffle_epi8(left, digits);
        let terms = _mm_castpd_si128(_mm_move_sd(
            _mm_castsi128_pd(left_terms),
            _mm_castsi128_pd(right_terms),
        ));
        sums = _mm_add_epi64(sums, _mm_sad_epu8(terms, zero));
    }
    Some([
        _mm_cvtsi128_si32(sums) as u32,
        _mm_cvtsi128_si32(_mm_unpackhi_epi64(sums, sums)) as u32,
    ])
}

#[cfg(all(
    target_arch = "aarch64",
    target_endian = "little",
    target_feature = "neon"
))]
fn arm_blocks(blocks: &[[u8; 16]], tables: &Tables) -> Option<[u64; 2]> {
    use std::arch::aarch64::*;
    // SAFETY: NEON is enabled for the target and every input array is 16 bytes.
    unsafe {
        let [right, left] = tables.each_ref().map(|t| vld1q_u8(t.as_ptr()));
        let even = vreinterpretq_u8_u16(vdupq_n_u16(0xff));
        let mut sums = [0; 2];
        for block in blocks {
            let d = vsubq_u8(vld1q_u8(block.as_ptr()), vdupq_n_u8(b'0'));
            if vmaxvq_u8(d) > 9 {
                return None;
            }
            sums[0] += u64::from(vaddlvq_u8(vbicq_u8(vqtbl1q_u8(right, d), even)));
            sums[1] += u64::from(vaddlvq_u8(vandq_u8(vqtbl1q_u8(left, d), even)));
        }
        Some(sums)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_aligned_maps_match_scalar_folds_at_every_byte_and_vector_boundary() {
        let tables: Tables = [
            std::array::from_fn(|i| (255 - i) as u8),
            std::array::from_fn(|i| (19 * i % 256) as u8),
        ];
        for length in 0..66 {
            let mut text: Vec<_> = (0..length).map(|i| b'0' + (i % 10) as u8).collect();
            let mut expected = [0; 2];
            for (i, &byte) in text.iter().rev().enumerate() {
                expected[i % 2] += u64::from(tables[i % 2][usize::from(byte - b'0')]);
            }
            assert_eq!(totals(&text, &tables), Some(expected));
            for pos in 0..length {
                let original = text[pos];
                for byte in 0..=u8::MAX {
                    if !byte.is_ascii_digit() {
                        text[pos] = byte;
                        assert_eq!(totals(&text, &tables), None);
                    }
                }
                text[pos] = original;
            }
        }
    }
}
