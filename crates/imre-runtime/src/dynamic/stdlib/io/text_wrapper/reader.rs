use super::{Codec, Context, Id, NONE, Result, Value};

/// Keep incomplete encoding sequences and CRLF pairs across arbitrary binary read boundaries.
pub(super) struct Reader {
    text: String,
    cursor: usize,
    tail: Vec<u8>,
    cr: bool,
    beginning: bool,
    seen: u8,
    // Most UTF-8 characters have the same size in the input and in our text buffer.
    // Keep only positions at which those sizes differ or the newline decoder has state.
    boundaries: Vec<Boundary>,
    mapped: bool,
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
            tail: Vec::new(),
            cr: false,
            beginning,
            seen: 0,
            boundaries: Vec::new(),
            mapped: true,
        }
    }
    pub(super) fn discard(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.tail.clear();
        self.cr = false;
        self.boundaries.clear();
        self.mapped = true;
    }
    pub(super) fn position(&self, ctx: &mut Context, end: Id) -> Result<Id> {
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
        Ok(ctx.int(raw - (rest.len() + self.tail.len() + usize::from(self.cr)) as i64 - extra))
    }
    pub(super) fn chunk(
        &mut self,
        bytes: &[u8],
        codec: Codec,
        end: bool,
    ) -> Result<(Codec, Vec<u8>)> {
        let mut raw = std::mem::take(&mut self.tail);
        raw.try_reserve(bytes.len())
            .map_err(|_| crate::Error::allocation())?;
        raw.extend_from_slice(bytes);
        let utf8 = matches!(codec, Codec::Utf8 { .. });
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
        let codec = if !self.beginning && matches!(codec, Codec::Utf8 { signature: true }) {
            Codec::Utf8 { signature: false }
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
        codec: Codec,
        newline: Option<&str>,
        end: bool,
    ) {
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
        let aligned = raw == decoded.as_bytes()
            || matches!(codec, Codec::Utf8 { signature: true })
                && raw.strip_prefix(b"\xef\xbb\xbf") == Some(decoded.as_bytes());
        let single_byte = !aligned
            && (matches!(codec, Codec::Latin1)
                || matches!(codec, Codec::CodePage(_))
                    && codec
                        .decode(raw, None)
                        .is_ok_and(|strict| strict == decoded));
        self.mapped &= aligned || single_byte;
        if matches!(newline, None | Some("")) {
            let translate = newline.is_none();
            for c in decoded.chars() {
                if self.cr {
                    self.cr = false;
                    if c == '\n' {
                        self.seen |= 4;
                        if !translate {
                            self.push('\r', 1, true);
                        }
                        self.push('\n', if translate { 2 } else { 1 }, false);
                        continue;
                    }
                    self.seen |= 1;
                    self.push(if translate { '\n' } else { '\r' }, 1, true);
                }
                if c == '\r' {
                    self.cr = true;
                } else {
                    if c == '\n' {
                        self.seen |= 2;
                    }
                    self.push(c, if single_byte { 1 } else { c.len_utf8() }, false);
                }
            }
            if end && self.cr {
                self.seen |= 1;
                self.push(if translate { '\n' } else { '\r' }, 1, true);
                self.cr = false;
            }
        } else if single_byte {
            for c in decoded.chars() {
                self.push(c, 1, false);
            }
        } else {
            self.text.push_str(decoded);
        }
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
        let mut found = Vec::new();
        for (bit, s) in [(1, "\r"), (2, "\n"), (4, "\r\n")] {
            if self.seen & bit != 0 {
                found.push(ctx.text(s));
            }
        }
        match found.as_slice() {
            [] => NONE,
            [id] => *id,
            _ => ctx.alloc(Value::Tuple(found)),
        }
    }
}
