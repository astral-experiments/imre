use super::{Codec, Context, FixedUnicode, Id, Result, StreamCodec};
use crate::objects::Utf16Order;

/// Keep incomplete encoding sequences and CRLF pairs across arbitrary binary read boundaries.
pub(super) struct Reader {
    text: String,
    cursor: usize,
    // CPython distinguishes no decoded chunk from an empty chunk at sized-read EOF.
    pub(super) decoded_chunk_nonempty: Option<bool>,
    pub(super) tail: Vec<u8>,
    cr: bool,
    pub(super) beginning: bool,
    pub(super) order: i32,
    pub(super) wide_snapshot: bool,
    seen: u8,
    // Most UTF-8 characters have the same size in the input and in our text buffer.
    // Keep only positions at which those sizes differ or the newline decoder has state.
    boundaries: Vec<Boundary>,
    mapped: bool,
    chunk_start: usize,
    snapshot: Snapshot,
}

enum Snapshot {
    None,
    Current { bytes: usize, tail: usize },
    // Iteration has replaced the decoded chunk but tell() still uses the last saved input.
    Previous(Option<ChunkOffsets>),
}

struct ChunkOffsets {
    bytes: usize,
    chars: usize,
    extra: Vec<(usize, usize)>,
}

struct Boundary {
    end: usize,
    extra_bytes: i8,
    decoder_state: bool,
}

impl Default for Reader {
    fn default() -> Self {
        Self::at(true)
    }
}

impl Reader {
    pub(super) fn at(beginning: bool) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            decoded_chunk_nonempty: None,
            tail: Vec::new(),
            cr: false,
            beginning,
            order: 0,
            wide_snapshot: false,
            seen: 0,
            boundaries: Vec::new(),
            mapped: true,
            chunk_start: 0,
            snapshot: Snapshot::None,
        }
    }
    pub(super) fn final_chunk_has_text(&self) -> bool {
        self.text.len() > self.chunk_start
    }
    pub(super) fn reset_decoder(&mut self) {
        self.tail.clear();
        self.cr = false;
        self.seen = 0;
        self.order = 0;
        self.beginning = true;
    }
    pub(super) fn after_read_all(&mut self) {
        // CPython clears decoded_chars on a full read or write only with a saved snapshot.
        let retained = matches!(self.snapshot, Snapshot::None)
            .then_some(self.decoded_chunk_nonempty)
            .flatten();
        self.discard();
        self.decoded_chunk_nonempty = retained;
    }
    pub(super) fn after_write(&mut self) {
        // An unseekable stream can still have unread decoded characters after a write.
        if !matches!(self.snapshot, Snapshot::None) {
            self.discard();
        }
        self.reset_decoder();
    }
    pub(super) fn discard(&mut self) {
        self.decoded_chunk_nonempty = None;
        self.text.clear();
        self.cursor = 0;
        self.tail.clear();
        self.cr = false;
        self.boundaries.clear();
        self.mapped = true;
        self.chunk_start = 0;
        self.clear_snapshot();
    }
    pub(super) fn clear_snapshot(&mut self) {
        self.snapshot = Snapshot::None;
        self.wide_snapshot = false;
    }
    pub(super) fn after_seek(beginning: bool, absolute: bool) -> Self {
        let mut reader = Self::at(beginning);
        if absolute {
            reader.snapshot = Snapshot::Current { bytes: 0, tail: 0 };
        }
        reader
    }
    /// Save the old chunk's byte offsets only if iteration reads ahead past it.
    fn save_offsets(&self, bytes: usize, tail: usize) -> Result<Option<ChunkOffsets>> {
        if !self.mapped || self.cr {
            return Ok(None);
        }
        let mut extra = Vec::new();
        let mut raw = 0;
        let mut chars = 0;
        let mut bounds = self
            .boundaries
            .iter()
            .filter(|b| b.end > self.chunk_start)
            .peekable();
        for (i, c) in self.text[self.chunk_start..].char_indices() {
            chars += 1;
            let mut width = c.len_utf8();
            if let Some(b) = bounds.next_if(|b| b.end == self.chunk_start + i + width) {
                if b.decoder_state {
                    return Ok(None);
                }
                width = width
                    .checked_add_signed(isize::from(b.extra_bytes))
                    .unwrap();
            }
            raw += width;
            if width > 1 {
                extra
                    .try_reserve(1)
                    .map_err(|_| crate::Error::allocation())?;
                extra.push((chars, width - 1));
            }
        }
        Ok((raw + tail == bytes).then_some(ChunkOffsets {
            bytes,
            chars,
            extra,
        }))
    }
    pub(super) fn position(&self, ctx: &mut Context, end: Id, newline_width: usize) -> Result<Id> {
        match &self.snapshot {
            Snapshot::None => return Ok(end),
            // Absolute seek starts with an empty snapshot. After iteration there are no
            // saved bytes to replay; use the current chunk's supported byte position.
            // CPython's release-build estimate can extend beyond that empty input.
            Snapshot::Previous(Some(chunk)) if chunk.bytes == 0 => {}
            Snapshot::Previous(Some(chunk)) => {
                let used = self.text[self.chunk_start..self.cursor.max(self.chunk_start)]
                    .chars()
                    .count();
                if used > chunk.chars {
                    return Err(super::Error::new(
                        "OSError",
                        "can't reconstruct logical file position",
                    ));
                }
                let extra: usize = chunk
                    .extra
                    .iter()
                    .take_while(|(n, _)| *n <= used)
                    .map(|(_, extra)| extra)
                    .sum();
                let back = (chunk.bytes - used - extra) as i64;
                let position = ctx.as_integer(end)?.checked_sub(back).ok_or_else(|| {
                    super::Error::new("OverflowError", "text stream position out of range")
                })?;
                return Ok(ctx.int(position));
            }
            Snapshot::Previous(None) => {
                return Err(super::Error::new(
                    "NotImplementedError",
                    "native text position needs a decoder snapshot",
                ));
            }
            Snapshot::Current { .. } => {}
        }
        let rest = &self.text[self.cursor..];
        if self
            .boundaries
            .iter()
            .any(|b| b.end == self.cursor && b.decoder_state)
            || rest.is_empty() && self.cr
            || !rest.is_empty() && !self.mapped
        {
            return Err(super::Error::new(
                "NotImplementedError",
                "native text position needs a decoder snapshot",
            ));
        }
        if rest.is_empty() && self.tail.is_empty() {
            return Ok(end);
        }
        let extra: i64 = self
            .boundaries
            .iter()
            .filter(|b| b.end > self.cursor)
            .map(|b| i64::from(b.extra_bytes))
            .sum();
        let raw = ctx.as_integer(end)?;
        Ok(ctx.int(
            raw - (rest.len() + self.tail.len() + usize::from(self.cr) * newline_width) as i64
                - extra,
        ))
    }
    pub(super) fn save_tail(&mut self, tail: &[u8]) -> Result<()> {
        self.tail
            .try_reserve(tail.len().saturating_sub(self.tail.len()))
            .map_err(|_| crate::Error::allocation())?;
        self.tail.clear();
        self.tail.extend_from_slice(tail);
        Ok(())
    }

    pub(super) fn chunk(
        &mut self,
        bytes: &[u8],
        codec: StreamCodec,
        end: bool,
    ) -> Result<(StreamCodec, Vec<u8>)> {
        if let StreamCodec::Wide { format, bom } = codec {
            let mut raw = Vec::new();
            raw.try_reserve(
                self.tail
                    .len()
                    .checked_add(bytes.len())
                    .ok_or_else(crate::Error::allocation)?,
            )
            .map_err(|_| crate::Error::allocation())?;
            raw.extend_from_slice(&self.tail);
            raw.extend_from_slice(bytes);
            let codec = if bom && !self.beginning {
                StreamCodec::Wide {
                    format: format.with_order(self.order),
                    bom: false,
                }
            } else {
                codec
            };
            return Ok((codec, raw));
        }
        let mut raw = std::mem::take(&mut self.tail);
        raw.try_reserve(bytes.len())
            .map_err(|_| crate::Error::allocation())?;
        raw.extend_from_slice(bytes);
        let utf8 = matches!(codec, StreamCodec::Registered(Codec::Utf8 { .. }));
        if utf8
            && !end
            && !raw.is_empty()
            && let Some(start) = (raw.len().saturating_sub(3)..raw.len())
                .rev()
                .find(|&i| raw[i] & 0xc0 != 0x80)
            && std::str::from_utf8(&raw[start..]).is_err_and(|e| e.error_len().is_none())
        {
            self.tail = raw.split_off(start);
        }
        let codec = if !self.beginning
            && matches!(
                codec,
                StreamCodec::Registered(Codec::Utf8 { signature: true })
            ) {
            StreamCodec::Registered(Codec::Utf8 { signature: false })
        } else {
            codec
        };
        if !raw.is_empty() {
            self.beginning = false;
        }
        Ok((codec, raw))
    }

    fn push(&mut self, c: char, raw_width: usize, decoder_state: bool) {
        self.text.push(c);
        let extra_bytes = raw_width as i8 - c.len_utf8() as i8;
        if extra_bytes != 0 || decoder_state {
            self.boundaries.push(Boundary {
                end: self.text.len(),
                extra_bytes,
                decoder_state,
            });
        }
    }

    pub(super) fn append(
        &mut self,
        decoded: &str,
        raw: &[u8],
        codec: StreamCodec,
        newline: Option<&str>,
        end: bool,
        telling: bool,
    ) -> Result<()> {
        if !telling && let Snapshot::Current { bytes, tail } = self.snapshot {
            self.snapshot = Snapshot::Previous(self.save_offsets(bytes, tail)?);
        }
        if self.cursor != 0 {
            self.text.drain(..self.cursor);
            self.boundaries.retain_mut(|b| {
                if b.end <= self.cursor {
                    return false;
                }
                b.end -= self.cursor;
                true
            });
            self.cursor = 0;
        }
        if self.text.is_empty() {
            self.mapped = true;
        }
        self.chunk_start = self.text.len();
        let aligned = raw == decoded.as_bytes()
            || matches!(
                codec,
                StreamCodec::Registered(Codec::Utf8 { signature: true })
            ) && raw.strip_prefix(b"\xef\xbb\xbf") == Some(decoded.as_bytes());
        let single_byte = !aligned
            && match codec {
                StreamCodec::Registered(Codec::Latin1) => true,
                StreamCodec::Registered(page @ Codec::CodePage(_)) => {
                    page.decode(raw, None).is_ok_and(|strict| strict == decoded)
                }
                _ => false,
            };
        let wide = match codec {
            StreamCodec::Wide {
                format: FixedUnicode::Utf16(order),
                ..
            } => {
                let little = matches!(order, Utf16Order::Little);
                decoded
                    .encode_utf16()
                    .flat_map(|c| {
                        if little {
                            c.to_le_bytes()
                        } else {
                            c.to_be_bytes()
                        }
                    })
                    .eq(raw.iter().copied())
            }
            StreamCodec::Wide { format, .. } => decoded
                .chars()
                .flat_map(|c| {
                    let point = u32::from(c);
                    if matches!(format, FixedUnicode::Utf32Le) {
                        point.to_le_bytes()
                    } else {
                        point.to_be_bytes()
                    }
                })
                .eq(raw.iter().copied()),
            _ => false,
        };
        let width = |c: char| match codec {
            StreamCodec::Wide {
                format: FixedUnicode::Utf16(_),
                ..
            } if wide => c.len_utf16() * 2,
            StreamCodec::Wide { .. } if wide => 4,
            _ if single_byte => 1,
            _ => c.len_utf8(),
        };
        let universal = matches!(newline, None | Some(""));
        let pending = usize::from(self.cr);
        let length = decoded
            .len()
            .checked_add(pending)
            .ok_or_else(crate::Error::allocation)?;
        self.text
            .try_reserve(length)
            .map_err(|_| crate::Error::allocation())?;
        let boundaries = if universal || single_byte || wide {
            decoded
                .chars()
                .filter(|&c| universal && c == '\r' || width(c) != c.len_utf8())
                .count()
                + pending
        } else {
            0
        };
        self.boundaries
            .try_reserve(boundaries)
            .map_err(|_| crate::Error::allocation())?;
        self.mapped &= aligned || single_byte || wide;
        if universal {
            let translate = newline.is_none();
            for c in decoded.chars() {
                if self.cr {
                    self.cr = false;
                    if c == '\n' {
                        self.seen |= 4;
                        if !translate {
                            self.push('\r', width('\r'), true);
                        }
                        self.push('\n', width('\n') * if translate { 2 } else { 1 }, false);
                        continue;
                    }
                    self.seen |= 1;
                    self.push(if translate { '\n' } else { '\r' }, width('\r'), true);
                }
                if c == '\r' {
                    self.cr = true;
                } else {
                    if c == '\n' {
                        self.seen |= 2;
                    }
                    self.push(c, width(c), false);
                }
            }
            if end && self.cr {
                self.seen |= 1;
                self.push(if translate { '\n' } else { '\r' }, width('\r'), true);
                self.cr = false;
            }
        } else if single_byte || wide {
            for c in decoded.chars() {
                self.push(c, width(c), false);
            }
        } else {
            self.text.push_str(decoded);
        }
        if telling {
            self.snapshot = Snapshot::Current {
                bytes: raw.len() + self.tail.len(),
                tail: self.tail.len(),
            };
        }
        Ok(())
    }
    pub(super) fn take(
        &mut self,
        limit: Option<usize>,
        line: bool,
        newline: Option<&str>,
        end: bool,
    ) -> Result<Option<String>> {
        let text = &self.text[self.cursor..];
        let boundary = if line {
            match newline {
                Some("") => text
                    .find(['\r', '\n'])
                    .map(|i| i + if text[i..].starts_with("\r\n") { 2 } else { 1 }),
                v => {
                    let sep = v.unwrap_or("\n");
                    text.find(sep).map(|i| i + sep.len())
                }
            }
        } else {
            None
        };
        let line_end = boundary.unwrap_or(text.len());
        let counted = limit.and_then(|n| {
            text[..line_end]
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(line_end))
                .nth(n)
        });
        if counted.is_none() && boundary.is_none() && !end {
            return Ok(None);
        }
        let n = counted.unwrap_or(line_end);
        let mut part = String::new();
        part.try_reserve_exact(n)
            .map_err(|_| crate::Error::allocation())?;
        part.push_str(&text[..n]);
        self.cursor += n;
        Ok(Some(part))
    }
    pub(super) fn newlines(&self, ctx: &mut Context) -> Id {
        super::super::newline_history(ctx, self.seen)
    }
}
