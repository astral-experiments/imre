use super::{Codec, Context, Cookie, Error, Id, NATIVE_ORDER, Result, StreamCodec, Value};
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct Snapshot {
    pub flags: i32,
    pub input: Rc<Vec<u8>>,
}

/// Keep incomplete encoding sequences and CRLF pairs across arbitrary binary read boundaries.
pub(super) struct Reader {
    text: String,
    // Each surrogate occupies one U+FFFD in text. Offsets distinguish it from a real U+FFFD.
    surrogates: Vec<(usize, u32)>,
    cursor: usize,
    chunk_start: usize,
    // CPython distinguishes no decoded chunk from an empty chunk at sized-read EOF.
    pub(super) decoded_chunk_nonempty: Option<bool>,
    pub(super) tail: Vec<u8>,
    cr: bool,
    pub(super) beginning: bool,
    pub(super) order: i32,
    pub(super) seen: u8,
    pub(super) snapshot: Option<Snapshot>,
    seek_seed: bool,
    pub(super) bytes_per_char: f64,
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
            surrogates: Vec::new(),
            cursor: 0,
            chunk_start: 0,
            decoded_chunk_nonempty: None,
            tail: Vec::new(),
            cr: false,
            beginning,
            order: 0,
            seen: 0,
            snapshot: None,
            seek_seed: false,
            bytes_per_char: 0.0,
        }
    }

    pub(super) fn reset_decoder(&mut self) {
        self.tail.clear();
        self.cr = false;
        self.seen = 0;
        self.order = 0;
        self.beginning = true;
    }

    pub(super) fn after_read_all(&mut self) {
        // A full read clears decoded_chars only when there was a saved snapshot.
        let retained = self
            .snapshot
            .is_none()
            .then_some(self.decoded_chunk_nonempty)
            .flatten();
        self.discard();
        self.decoded_chunk_nonempty = retained;
    }

    pub(super) fn after_write(&mut self) {
        // An unseekable stream can still have unread decoded characters after a write.
        if self.snapshot.is_some() {
            self.discard();
        }
        self.reset_decoder();
    }

    pub(super) fn discard(&mut self) {
        self.text.clear();
        self.surrogates.clear();
        self.cursor = 0;
        self.chunk_start = 0;
        self.decoded_chunk_nonempty = None;
        self.tail.clear();
        self.cr = false;
        self.clear_snapshot();
    }

    pub(super) fn clear_snapshot(&mut self) {
        self.snapshot = None;
        self.seek_seed = false;
    }

    pub(super) fn flags(&self, codec: StreamCodec, newline: Option<&str>) -> i32 {
        let flags = match codec {
            StreamCodec::Wide { bom: true, .. } if self.beginning => 2,
            StreamCodec::Wide { bom: true, .. } => i32::from(self.order == -NATIVE_ORDER),
            StreamCodec::Registered(Codec::Utf8 { signature: true }) => i32::from(self.beginning),
            _ => 0,
        };
        if matches!(newline, None | Some("")) {
            (flags << 1) | i32::from(self.cr)
        } else {
            flags
        }
    }

    pub(super) fn set_state(&mut self, cookie: Cookie, codec: StreamCodec, newline: Option<&str>) {
        if cookie.start == 0 && cookie.flags == 0 {
            self.reset_decoder();
            return;
        }
        let universal = matches!(newline, None | Some(""));
        let flags = cookie.flags >> u32::from(universal);
        self.tail.clear();
        self.cr = universal && cookie.flags & 1 != 0;
        self.beginning = match codec {
            StreamCodec::Wide { bom: true, .. } => !matches!(flags, 0 | 1),
            StreamCodec::Registered(Codec::Utf8 { signature: true }) => flags != 0,
            _ => false,
        };
        self.order = if self.beginning {
            0
        } else if flags == 1 {
            -NATIVE_ORDER
        } else {
            NATIVE_ORDER
        };
    }

    pub(super) fn after_seek(&mut self, cookie: Cookie, codec: StreamCodec, newline: Option<&str>) {
        self.discard();
        self.set_state(cookie, codec, newline);
        if cookie.skip == 0 {
            self.snapshot = Some(Snapshot {
                flags: cookie.flags,
                input: Rc::default(),
            });
            self.seek_seed = true;
        }
    }

    pub(super) fn capture(
        &self,
        codec: StreamCodec,
        newline: Option<&str>,
        telling: bool,
    ) -> Result<Option<Snapshot>> {
        if !telling && !self.seek_seed {
            return Ok(None);
        }
        let mut input = Vec::new();
        input
            .try_reserve_exact(self.tail.len())
            .map_err(|_| crate::Error::allocation())?;
        input.extend_from_slice(&self.tail);
        Ok(Some(Snapshot {
            flags: self.flags(codec, newline),
            input: Rc::new(input),
        }))
    }

    pub(super) fn record(
        &mut self,
        snapshot: Option<Snapshot>,
        bytes: usize,
        chars: usize,
        telling: bool,
    ) {
        self.bytes_per_char = if chars == 0 {
            0.0
        } else {
            bytes as f64 / chars as f64
        };
        // After an absolute seek the saved input is empty. Keep actual read-ahead bytes
        // during iteration so reconstruction can never index beyond that empty input.
        if let Some(snapshot) = snapshot {
            self.snapshot = Some(snapshot);
        }
        if telling {
            self.seek_seed = false;
        }
    }

    pub(super) fn seek_input(&mut self, flags: i32, data: &[u8]) -> Result<()> {
        let mut input = Vec::new();
        input
            .try_reserve_exact(data.len())
            .map_err(|_| crate::Error::allocation())?;
        input.extend_from_slice(data);
        self.snapshot = Some(Snapshot {
            flags,
            input: Rc::new(input),
        });
        self.seek_seed = data.is_empty();
        Ok(())
    }

    /// Sized reads estimate the byte count from the previous successful decoder refill.
    pub(super) fn read_size(&self, minimum: usize, requested: usize) -> usize {
        let buffered = self.text[self.cursor..].chars().count();
        let remaining = requested.saturating_sub(buffered);
        let estimate = self.bytes_per_char.max(1.0) * remaining as f64;
        if estimate >= i64::MAX as f64 {
            return minimum;
        }
        minimum.max(estimate as usize)
    }

    pub(super) fn used_in_chunk(&self) -> usize {
        self.text[self.chunk_start..self.cursor.max(self.chunk_start)]
            .chars()
            .count()
    }

    pub(super) fn skip_chars(&mut self, count: usize) -> Result<()> {
        self.cursor = self
            .text
            .char_indices()
            .map(|(byte, _)| byte)
            .chain(std::iter::once(self.text.len()))
            .nth(count)
            .ok_or_else(|| Error::new("OSError", "can't restore logical file position"))?;
        Ok(())
    }

    pub(super) fn consume_chunk(&mut self) {
        self.cursor = self.text.len();
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
        if matches!(
            codec,
            StreamCodec::Wide { .. } | StreamCodec::Registered(Codec::DoubleByte(_))
        ) {
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
            let codec = if let StreamCodec::Wide { format, bom: true } = codec
                && !self.beginning
            {
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
        let signature = matches!(
            codec,
            StreamCodec::Registered(Codec::Utf8 { signature: true })
        );
        // UTF-8-SIG defers even final input while it is still a proper prefix of the BOM.
        if signature && self.beginning && raw.len() < 3 && b"\xef\xbb\xbf".starts_with(&raw) {
            self.tail = raw;
            return Ok((codec, Vec::new()));
        }
        let first = self.beginning;
        if !raw.is_empty() {
            self.beginning = false;
        }
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
        let codec = if !first
            && matches!(
                codec,
                StreamCodec::Registered(Codec::Utf8 { signature: true })
            ) {
            StreamCodec::Registered(Codec::Utf8 { signature: false })
        } else {
            codec
        };
        Ok((codec, raw))
    }

    fn push_translated(&mut self, c: char, translate: bool) {
        if self.cr {
            self.cr = false;
            if c == '\n' {
                self.seen |= 4;
                if !translate {
                    self.text.push('\r');
                }
                self.text.push('\n');
                return;
            }
            self.seen |= 1;
            self.text.push(if translate { '\n' } else { '\r' });
        }
        if c == '\r' {
            self.cr = true;
        } else {
            if c == '\n' {
                self.seen |= 2;
            }
            self.text.push(c);
        }
    }

    pub(super) fn append(
        &mut self,
        decoded: &Value,
        newline: Option<&str>,
        end: bool,
    ) -> Result<usize> {
        let (bytes, marks) = match decoded {
            Value::Str(s) => (s.len(), 0),
            Value::SurrogateStr(points) => {
                let mut bytes = 0;
                let mut marks = 0;
                for &point in points {
                    let c = char::from_u32(point);
                    marks += usize::from(c.is_none());
                    bytes += c.unwrap_or(char::REPLACEMENT_CHARACTER).len_utf8();
                }
                (bytes, marks)
            }
            _ => unreachable!("validated decoded text"),
        };
        let bytes = bytes
            .checked_add(usize::from(self.cr))
            .ok_or_else(crate::Error::allocation)?;
        self.text
            .try_reserve(bytes.saturating_sub(self.cursor))
            .map_err(|_| crate::Error::allocation())?;
        let consumed = self.surrogates.partition_point(|&(at, _)| at < self.cursor);
        self.surrogates
            .try_reserve(marks.saturating_sub(consumed))
            .map_err(|_| crate::Error::allocation())?;
        if self.cursor != 0 {
            self.text.drain(..self.cursor);
            self.surrogates.drain(..consumed);
            for (at, _) in &mut self.surrogates {
                *at -= self.cursor;
            }
            self.cursor = 0;
        }
        self.chunk_start = self.text.len();
        let universal = matches!(newline, None | Some(""));
        let translate = newline.is_none();
        match decoded {
            Value::Str(s) if !universal => self.text.push_str(s),
            Value::Str(s) => {
                let mut rest: &str = s;
                while !rest.is_empty() {
                    if self.cr || matches!(rest.as_bytes()[0], b'\r' | b'\n') {
                        let c = rest.chars().next().expect("nonempty decoded text");
                        self.push_translated(c, translate);
                        rest = &rest[c.len_utf8()..];
                    } else {
                        let n =
                            memchr::memchr2(b'\r', b'\n', rest.as_bytes()).unwrap_or(rest.len());
                        self.text.push_str(&rest[..n]);
                        rest = &rest[n..];
                    }
                }
            }
            Value::SurrogateStr(points) => {
                for &point in points {
                    let scalar = char::from_u32(point);
                    let c = scalar.unwrap_or(char::REPLACEMENT_CHARACTER);
                    if universal {
                        self.push_translated(c, translate);
                    } else {
                        self.text.push(c);
                    }
                    if scalar.is_none() {
                        self.surrogates
                            .push((self.text.len() - c.len_utf8(), point));
                    }
                }
            }
            _ => unreachable!("validated decoded text"),
        }
        if universal && end && self.cr {
            self.seen |= 1;
            self.text.push(if translate { '\n' } else { '\r' });
            self.cr = false;
        }
        Ok(self.text[self.chunk_start..].chars().count())
    }
    pub(super) fn take(
        &mut self,
        limit: Option<usize>,
        line: bool,
        newline: Option<&str>,
        end: bool,
    ) -> Result<Option<Value>> {
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
        let text = &text[..n];
        let first = self.surrogates.partition_point(|&(at, _)| at < self.cursor);
        let last = self
            .surrogates
            .partition_point(|&(at, _)| at < self.cursor + n);
        let part = if first == last {
            let mut part = String::new();
            part.try_reserve_exact(n)
                .map_err(|_| crate::Error::allocation())?;
            part.push_str(text);
            Value::Str(part.into())
        } else {
            let mut points = Vec::new();
            points
                .try_reserve_exact(text.chars().count())
                .map_err(|_| crate::Error::allocation())?;
            let mut marks = self.surrogates[first..last].iter().copied().peekable();
            for (offset, c) in text.char_indices() {
                let point = marks.next_if(|&(at, _)| at == self.cursor + offset);
                points.push(point.map_or(u32::from(c), |(_, point)| point));
            }
            Value::SurrogateStr(points)
        };
        self.cursor += n;
        Ok(Some(part))
    }
    pub(super) fn newlines(&self, ctx: &mut Context) -> Id {
        super::super::newline_history(ctx, self.seen)
    }
}
