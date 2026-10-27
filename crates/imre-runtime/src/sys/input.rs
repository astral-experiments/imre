//! Keep the incremental decoder and unread text with the process's standard input.
use super::encoding;
use crate::objects::{Codec, errno_error};
use crate::{Error, Kind, Result};
use std::io::{self, Read};
use std::sync::{LazyLock, Mutex};

// TextIOWrapper reads up to this many bytes before it returns a line. A decode
// error in the same chunk precedes even a newline already present in the chunk.
const CHUNK: usize = 8192;
static INPUT: LazyLock<Mutex<Input>> = LazyLock::new(|| Mutex::new(Input::default()));

#[derive(Default)]
struct Input {
    text: String,
    offset: usize,
    pending: Vec<u8>,
    started: bool,
    eof: bool,
}

/// A UTF-8 character split by a raw read belongs to the next decoding call.
fn complete_prefix(bytes: &[u8]) -> usize {
    for start in bytes.len().saturating_sub(3)..bytes.len() {
        if let Err(error) = str::from_utf8(&bytes[start..])
            && error.valid_up_to() == 0
            && error.error_len().is_none()
        {
            return start;
        }
    }
    bytes.len()
}

impl Input {
    fn fill(&mut self, reader: &mut impl Read, settings: &encoding::Settings) -> Result<()> {
        let mut bytes = [0; CHUNK + 3];
        let prefix = self.pending.len();
        bytes[..prefix].copy_from_slice(&self.pending);
        let count = loop {
            match reader.read(&mut bytes[prefix..prefix + CHUNK]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => break result.map_err(errno_error)?,
            }
        };
        self.eof = count == 0;
        let data = &bytes[..prefix + count];
        let ready = if !self.eof && matches!(settings.codec, Codec::Utf8 { .. }) {
            complete_prefix(data)
        } else {
            data.len()
        };
        self.text.clear();
        self.offset = 0;
        if ready != 0 {
            let codec = match settings.codec {
                Codec::Utf8 { signature: true } if self.started => Codec::Utf8 { signature: false },
                codec => codec,
            };
            self.started = true;
            self.text = codec.decode(&data[..ready], Some(&settings.errors))?;
        }
        // A failed incremental decode preserves the previously buffered prefix,
        // without accepting any suffix from the raw read that just failed.
        self.pending.clear();
        self.pending.extend_from_slice(&data[ready..]);
        Ok(())
    }

    fn line(&mut self, reader: &mut impl Read) -> Result<String> {
        let settings = encoding::settings(false, None);
        let mut line = String::new();
        // A terminal can produce EOF for a single read and supply another line later.
        self.eof = false;
        loop {
            let remaining = &self.text[self.offset..];
            if let Some(newline) = remaining.find('\n') {
                line.push_str(&remaining[..newline]);
                self.offset += newline + 1;
                return Ok(line);
            }
            line.push_str(remaining);
            self.offset = self.text.len();
            if self.eof {
                return if line.is_empty() {
                    Err(Error::new(Kind::EOFError, "EOF when reading a line"))
                } else {
                    Ok(line)
                };
            }
            self.fill(reader, settings)?;
        }
    }
}

pub(crate) fn read_input_line() -> Result<String> {
    // A preceding print must be visible before waiting for the next input line.
    crate::flush_stdout().map_err(errno_error)?;
    let mut input = INPUT.lock().unwrap_or_else(|poison| poison.into_inner());
    input.line(&mut io::stdin().lock())
}
