//! The text and binary buffering rules shared by Python files and standard streams.

use std::io::{self, Write};

/// Bound the descriptor's preferred block size as CPython does for buffered files.
pub(crate) fn binary_capacity(block_size: i128) -> usize {
    block_size.clamp(128 * 1024, 8 * 1024 * 1024) as usize
}

/// TextIOWrapper groups small encoded writes and forwards a large write separately.
#[derive(Default)]
pub(crate) struct PendingText(Vec<u8>);

const CHUNK_SIZE: usize = 8192;
impl PendingText {
    /// Append a whole fragmented write only if it fits without allocation or a flush.
    pub(crate) fn try_append_parts<'a>(&mut self, parts: impl Iterator<Item = &'a [u8]>) -> bool {
        let initial = self.0.len();
        let Some(room) = self.0.capacity().min(CHUNK_SIZE - 1).checked_sub(initial) else {
            return false;
        };
        let mut remaining = &mut self.0.spare_capacity_mut()[..room];
        for part in parts {
            let Some((destination, rest)) = remaining.split_at_mut_checked(part.len()) else {
                return false;
            };
            destination.write_copy_of_slice(part);
            remaining = rest;
        }
        let written = room - remaining.len();
        // SAFETY: every fragment initialized its consecutive spare slots. Failed writes
        // leave the original length visible and never publish a partial fragment.
        unsafe { self.0.set_len(initial + written) };
        true
    }

    pub(crate) fn write<W: Write>(
        &mut self,
        raw: &mut W,
        bytes: &[u8],
        flush_line: bool,
    ) -> io::Result<()> {
        if bytes.len() >= CHUNK_SIZE {
            self.drain(raw)?;
            raw.write_all(bytes)?;
        } else {
            self.0
                .try_reserve(bytes.len())
                .map_err(|_| io::ErrorKind::OutOfMemory)?;
            self.0.extend_from_slice(bytes);
            if self.0.len() >= CHUNK_SIZE || flush_line {
                self.drain(raw)?;
            }
        }
        if flush_line {
            raw.flush()?;
        }
        Ok(())
    }

    fn drain<W: Write>(&mut self, raw: &mut W) -> io::Result<()> {
        let result = raw.write_all(&self.0);
        // TextIOWrapper relinquishes pending text even when the binary write fails.
        self.0.clear();
        result
    }

    pub(crate) fn flush<W: Write>(&mut self, raw: &mut W) -> io::Result<()> {
        self.drain(raw)?;
        raw.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_writes_require_space_for_every_part() {
        let mut pending = PendingText(Vec::with_capacity(12));
        assert!(pending.try_append_parts([b"abc".as_slice(), b"def"].into_iter()));
        assert!(!pending.try_append_parts([b"ghi".as_slice(), b"too long"].into_iter()));
        assert_eq!(pending.0, b"abcdef");

        let mut pending = PendingText(Vec::with_capacity(CHUNK_SIZE));
        pending.0.resize(CHUNK_SIZE - 2, b'x');
        assert!(pending.try_append_parts([b"y".as_slice()].into_iter()));
        assert!(!pending.try_append_parts([b"z".as_slice()].into_iter()));
        assert_eq!(pending.0.len(), CHUNK_SIZE - 1);
        assert_eq!(pending.0.last(), Some(&b'y'));
    }
}
