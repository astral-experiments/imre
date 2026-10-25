use super::{Str, Words};
use crate::StrOwner;
use std::cmp::Ordering;

/// Each UTF-8 row stores its common byte prefix with the previous row and its
/// remaining bytes. A two-bit side tag lets equal rows serve both inputs.
pub(super) struct Packed {
    blocks: Vec<Block>,
    counts: (usize, usize),
}

const BLOCK_BYTES: usize = 64 * 1024;
// A side tag and two base-128 usize lengths.
const MAX_ROW_HEADER_BYTES: usize = 1 + 2 * usize::BITS.div_ceil(7) as usize;

enum Block {
    Raw(Box<[u8]>),
    Lz4 {
        bytes: Box<[u8]>,
        decoded_len: usize,
    },
}

impl Block {
    fn bytes<'a>(&'a self, scratch: &'a mut Vec<u8>) -> &'a [u8] {
        match self {
            Self::Raw(bytes) => bytes,
            Self::Lz4 { bytes, decoded_len } => {
                scratch.resize(*decoded_len, 0);
                let len = lz4_flex::block::decompress_into(bytes, scratch)
                    .expect("valid internal snapshot block");
                assert_eq!(len, *decoded_len);
                scratch
            }
        }
    }
}

fn flush(blocks: &mut Vec<Block>, data: &mut Vec<u8>) {
    if data.is_empty() {
        return;
    }
    if data.len() <= BLOCK_BYTES {
        let bytes = lz4_flex::block::compress(data);
        if bytes.len() < data.len() {
            blocks.push(Block::Lz4 {
                bytes: bytes.into_boxed_slice(),
                decoded_len: data.len(),
            });
            data.clear();
            return;
        }
    }
    // A single very large row stays in its own uncompressed block.
    blocks.push(Block::Raw(std::mem::take(data).into_boxed_slice()));
}

/// Finds the first differing byte with unaligned, endian-independent word reads.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let len = a.len().min(b.len());
    let (a_words, a_tail) = a[..len].as_chunks::<8>();
    let (b_words, b_tail) = b[..len].as_chunks::<8>();
    for (index, (left, right)) in a_words.iter().zip(b_words).enumerate() {
        let difference = u64::from_le_bytes(*left) ^ u64::from_le_bytes(*right);
        if difference != 0 {
            return index * 8 + difference.trailing_zeros() as usize / 8;
        }
    }
    a_words.len() * 8
        + a_tail
            .iter()
            .zip(b_tail)
            .take_while(|(l, r)| l == r)
            .count()
}

fn put(mut n: usize, bytes: &mut Vec<u8>) {
    while n >= 128 {
        bytes.push((n as u8) | 128);
        n >>= 7;
    }
    bytes.push(n as u8);
}

fn take(bytes: &mut &[u8]) -> usize {
    let mut value = 0;
    for shift in (0..usize::BITS).step_by(7) {
        let (&byte, rest) = bytes.split_first().expect("complete snapshot varint");
        *bytes = rest;
        value |= usize::from(byte & 127) << shift;
        if byte < 128 {
            return value;
        }
    }
    unreachable!("snapshot lengths fit usize")
}

impl Packed {
    pub(super) fn new(left: &Words, right: &Words) -> Self {
        let counts = (left.values.len(), right.values.len());
        let mut left = left.strings().map(|(_, s)| s.as_bytes()).peekable();
        let mut right = right.strings().map(|(_, s)| s.as_bytes()).peekable();
        let mut data = Vec::new();
        let mut blocks = Vec::new();
        let mut previous: &[u8] = &[];
        loop {
            let order = match (left.peek(), right.peek()) {
                (None, None) => break,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(l), Some(r)) => l.cmp(r),
            };
            let (side, row) = match order {
                Ordering::Less => (1, left.next().unwrap()),
                Ordering::Greater => (2, right.next().unwrap()),
                Ordering::Equal => {
                    right.next();
                    (3, left.next().unwrap())
                }
            };
            let prefix = if std::ptr::eq(previous, row) {
                row.len()
            } else {
                common_prefix(previous, row)
            };
            // A whole row fits in the decode scratch, or occupies one raw block.
            // Its prefix can refer to the last row of the preceding block.
            let suffix = &row[prefix..];
            if BLOCK_BYTES.saturating_sub(data.len()) < MAX_ROW_HEADER_BYTES + suffix.len() {
                flush(&mut blocks, &mut data);
            }
            data.push(side);
            put(prefix, &mut data);
            put(row.len() - prefix, &mut data);
            data.extend_from_slice(suffix);
            previous = row;
        }
        flush(&mut blocks, &mut data);
        Self { blocks, counts }
    }

    pub(super) fn unpack(&self) -> (Words, Words) {
        let (mut left, mut right) = (Words::new(3), Words::new(3));
        left.values.reserve(self.counts.0);
        right.values.reserve(self.counts.1);
        let mut decoded = Vec::new();
        let mut row = Vec::new();
        let mut string = StrOwner::new(Str::new(""));
        for block in &self.blocks {
            let mut data = block.bytes(&mut decoded);
            while let Some((&side, rest)) = data.split_first() {
                data = rest;
                let prefix = take(&mut data);
                let len = take(&mut data);
                let (suffix, rest) = data.split_at(len);
                data = rest;
                // A repeated large string must not gain one allocation per occurrence.
                if prefix != row.len() || len != 0 {
                    row.truncate(prefix);
                    row.extend_from_slice(suffix);
                    string = StrOwner::new(Str::new(
                        std::str::from_utf8(&row).expect("complete UTF-8 row"),
                    ));
                }
                if side & 1 != 0 {
                    left.values
                        .push(StrOwner::into_raw(StrOwner::clone(&string)) as usize);
                }
                if side & 2 != 0 {
                    right
                        .values
                        .push(StrOwner::into_raw(StrOwner::clone(&string)) as usize);
                }
            }
        }
        left.update_header();
        right.update_header();
        (left, right)
    }
}

#[cfg(test)]
mod tests;
