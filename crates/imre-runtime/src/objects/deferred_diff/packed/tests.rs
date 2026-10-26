use super::*;
use crate::StrOwner;

#[test]
fn prefix_handles_mismatches_in_every_word_and_tail_byte() {
    let bytes: Vec<_> = (0..64u8).map(|n| n.wrapping_mul(103)).collect();
    for offset in 0..8 {
        let longer = &bytes[offset..];
        for len in 0..32 {
            let a = &longer[..len];
            let mut b = [0; 32];
            b[..len].copy_from_slice(a);
            assert_eq!(common_prefix(a, &b[..len]), len);
            assert_eq!(common_prefix(longer, &b[..len]), len);
            assert_eq!(common_prefix(&b[..len], longer), len);
            for i in 0..len {
                b[i] ^= 0x80;
                assert_eq!(common_prefix(a, &b[..len]), i);
                b[i] ^= 0x80;
            }
        }
    }
}

fn words(rows: &[String]) -> Words {
    let mut result = Words::new(3);
    for row in rows {
        result
            .values
            .push(StrOwner::into_raw(StrOwner::new(Str::new(row))) as usize);
    }
    result.update_header();
    result
}

#[test]
fn compressed_and_raw_blocks_preserve_cross_block_prefixes_and_duplicates() {
    let mut left: Vec<_> = (0..3500)
        .map(|i| format!("a{i:07}: repeated UTF-8 漢\0 with a predictable suffix"))
        .collect();
    // A deterministic, incompressible row exercises the bounded raw fallback.
    let mut state = 71u32;
    let mut noise = String::from("z");
    for _ in 0..60_000 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        noise.push(char::from(32 + (state % 95) as u8));
    }
    left.push(noise);
    let mut right = left.clone();
    let jumbo = "🦀".repeat(40_000);
    let first = format!("{jumbo}é");
    let next = format!("{jumbo}ê");
    right.extend([first.clone(), next.clone()]);
    left.extend([first.clone(), first, next]);

    let packed =
        Packed::new(&words(&left), &words(&right)).unwrap_or_else(|_| panic!("pack test rows"));
    assert!(
        packed
            .blocks
            .iter()
            .filter(|b| matches!(b, Block::Lz4 { .. }))
            .count()
            >= 2
    );
    assert!(
        packed
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Raw(s) if s.len() <= BLOCK_BYTES))
    );
    assert!(
        packed
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Raw(s) if s.len() > BLOCK_BYTES))
    );

    let (a, b) = packed
        .unpack()
        .unwrap_or_else(|_| panic!("restore packed rows"));
    assert!(
        a.strings()
            .map(|(_, s)| s.as_str())
            .eq(left.iter().map(String::as_str))
    );
    assert!(
        b.strings()
            .map(|(_, s)| s.as_str())
            .eq(right.iter().map(String::as_str))
    );
    // The first jumbo row ends its block. Its repeat on the left and the
    // corresponding row on the right share the restored owner.
    assert_eq!(a.values[left.len() - 3], a.values[left.len() - 2]);
    assert_eq!(a.values[left.len() - 2], b.values[right.len() - 2]);
}
