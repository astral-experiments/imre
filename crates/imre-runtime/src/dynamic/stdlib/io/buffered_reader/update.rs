//! A callback-backed BufferedRandom keeps pending writes in its read-ahead window.
use super::super::{bytes_io, bytes_writer, descriptor_buffer};
use super::{Cell, Context, Error, Id, NONE, Reader, Result, Value, index, invoke};

#[derive(Default)]
pub(super) struct Update {
    pub(super) raw_offset: Cell<usize>,
    // Bytes outside this range were read from the raw stream or have already been written.
    pub(super) dirty: Cell<Option<(usize, usize)>>,
}

impl Reader {
    pub(super) fn raw_offset(&self) -> usize {
        self.update
            .as_ref()
            .map_or(self.end.get(), |u| u.raw_offset.get())
    }

    fn move_raw(&self, ctx: &mut Context, update: &Update, to: usize, force: bool) -> Result<()> {
        if !force && update.raw_offset.get() == to {
            return Ok(());
        }
        let offset = ctx.int(to as i64 - update.raw_offset.get() as i64);
        let whence = ctx.int(1);
        let result = invoke(ctx, self.raw()?, "seek", vec![offset, whence])?;
        let pos = index(ctx, result)?;
        if pos < 0 {
            return Err(Error::new(
                "OSError",
                format!("Raw stream returned invalid position {pos}"),
            ));
        }
        self.absolute.set(Some(pos));
        update.raw_offset.set(to);
        Ok(())
    }

    pub(super) fn write_back(&self, ctx: &mut Context) -> Result<()> {
        let Some(update) = &self.update else {
            return Ok(());
        };
        let Some((mut start, end)) = update.dirty.get() else {
            return Ok(());
        };
        self.move_raw(ctx, update, start, false)?;
        while start < end {
            let data = bytes_io::copy_bytes(&ctx.bytes_like(self.buffer)?[start..end])?;
            let Some(n) = bytes_writer::raw_write(ctx, self.raw()?, &data)? else {
                return Err(descriptor_buffer::blocking_error(ctx, 0, 0));
            };
            start += n;
            self.advance(n);
            update.raw_offset.set(start);
            update.dirty.set(Some((start, end)));
        }
        update.dirty.set(None);
        Ok(())
    }

    pub(super) fn sync_update(&self, ctx: &mut Context) -> Result<()> {
        if let Some(update) = &self.update {
            self.write_back(ctx)?;
            // BufferedRandom rewinds even at offset zero. Raw callbacks observe this seek.
            let result = self.move_raw(ctx, update, self.pos.get(), true);
            self.reset();
            result?;
        }
        Ok(())
    }

    fn buffer_input(
        &self,
        ctx: &mut Context,
        input: Id,
        offset: usize,
        count: usize,
        update: &Update,
    ) -> Result<()> {
        if count == 0 {
            return Ok(());
        }
        let bytes = bytes_io::copy_bytes(&ctx.native_buffer_bytes(input)?[offset..offset + count])?;
        let start = self.pos.get();
        let end = start + count;
        let Value::ByteArray(buffer) = ctx.heap.get_mut(self.buffer) else {
            unreachable!()
        };
        buffer[start..end].copy_from_slice(&bytes);
        let (first, last) = update.dirty.get().unwrap_or((start, end));
        update.dirty.set(Some((first.min(start), last.max(end))));
        self.pos.set(end);
        if self.end.get() != 0 {
            self.end.set(self.end.get().max(end));
        }
        Ok(())
    }

    fn write_input(&self, ctx: &mut Context, input: Id, update: &Update) -> Result<Id> {
        let size = ctx.native_buffer_bytes(input)?.len();
        if size < self.size && size <= self.size - self.pos.get() {
            self.buffer_input(ctx, input, 0, size, update)?;
            return Ok(ctx.int(size as i64));
        }
        if let Err(error) = self.write_back(ctx) {
            let blocking = ctx.builtin("BlockingIOError");
            if !ctx.raised_error_matches(&error, blocking) {
                return Err(error);
            }
            // Make space occupied by previously written bytes available to the new input.
            let Some((start, end)) = update.dirty.get() else {
                return Err(error);
            };
            let pos = self.pos.get();
            if start > pos {
                return Err(error);
            }
            let Value::ByteArray(buffer) = ctx.heap.get_mut(self.buffer) else {
                unreachable!()
            };
            buffer.copy_within(start..end, 0);
            self.end.set(0);
            self.pos.set(pos - start);
            update
                .raw_offset
                .set(update.raw_offset.get().saturating_sub(start));
            update.dirty.set(Some((0, end - start)));
            let logical = self.pos.get();
            self.pos.set(end - start);
            let accepted = size.min(self.size - self.pos.get());
            self.buffer_input(ctx, input, 0, accepted, update)?;
            self.pos.set(logical + accepted);
            return if accepted == size {
                Ok(ctx.int(size as i64))
            } else {
                Err(descriptor_buffer::blocking_error(ctx, 0, accepted))
            };
        }
        self.move_raw(ctx, update, self.pos.get(), false)?;
        self.reset();
        let mut written = 0;
        while size - written >= self.size {
            let data = bytes_io::copy_bytes(&ctx.native_buffer_bytes(input)?[written..])?;
            let Some(n) = bytes_writer::raw_write(ctx, self.raw()?, &data)? else {
                if size - written == self.size {
                    break;
                }
                self.buffer_input(ctx, input, written, self.size, update)?;
                return Err(descriptor_buffer::blocking_error(
                    ctx,
                    0,
                    written + self.size,
                ));
            };
            written += n;
            self.advance(n);
        }
        self.buffer_input(ctx, input, written, size - written, update)?;
        Ok(ctx.int(size as i64))
    }

    pub(super) fn update_call(&self, ctx: &mut Context, op: &str, args: &[Id]) -> Result<Id> {
        if op == "write" {
            return self.with_lock(|| {
                // The raw closed property can call this same writer.
                self.check_open(ctx, "write to closed file")?;
                self.write_input(ctx, args[0], self.update.as_ref().unwrap())
            });
        }
        self.check_open(
            ctx,
            if op == "flush" {
                "flush of closed file"
            } else {
                "truncate of closed file"
            },
        )?;
        self.with_lock(|| {
            self.sync_update(ctx)?;
            if op == "truncate" {
                let raw = self.raw()?;
                let result = invoke(ctx, raw, op, vec![args.first().copied().unwrap_or(NONE)])?;
                self.absolute.set(
                    invoke(ctx, raw, "tell", vec![])
                        .and_then(|v| index(ctx, v))
                        .ok()
                        .filter(|p| *p >= 0),
                );
                return Ok(result);
            }
            Ok(NONE)
        })
    }
}
