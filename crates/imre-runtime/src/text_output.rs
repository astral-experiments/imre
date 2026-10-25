//! The text and binary buffering rules shared by Python files and standard streams.

use std::io::{self, Write};

/// Bound the descriptor's preferred block size as CPython does for buffered files.
pub(crate) fn binary_capacity(block_size: i128) -> usize {
    block_size.clamp(128 * 1024, 8 * 1024 * 1024) as usize
}

/// TextIOWrapper groups small encoded writes and forwards a large write separately.
#[derive(Default)]
pub(crate) struct PendingText(Vec<u8>);

impl PendingText {
    pub(crate) fn write<W: Write>(
        &mut self,
        raw: &mut W,
        bytes: &[u8],
        flush_line: bool,
    ) -> io::Result<()> {
        const CHUNK_SIZE: usize = 8192;
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
