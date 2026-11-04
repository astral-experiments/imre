//! Boxed gzip access to the lazy path reader and incremental DEFLATE encoder.

use std::cell::{Cell, RefCell};
use std::io::{self, Read};

use super::super::value::BoxedGzipDecoder;
use super::{Context, Error, Id, NONE, Result, Value, os};
use crate::objects::{FileState, GzipDecoder};

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let arity = match name {
        "decoder" => 0,
        "raw_read" => 3,
        "read" | "read1" | "compress" | "flush_writer" => 2,
        "readline" if args.len() == 2 => 2,
        _ => 1,
    };
    if !kwargs.is_empty() || args.len() != arity {
        return Err(Error::new(
            "TypeError",
            format!("_imre_gzip.{name}() requires {arity} positional arguments"),
        ));
    }
    if matches!(name, "writer" | "compress" | "flush_writer") {
        return super::zlib::gzip_writer(ctx, name, args);
    }
    if name == "decoder" {
        let state = crate::Shared::try_new(BoxedGzipDecoder {
            stream: RefCell::new(Some(GzipDecoder::new())),
            last_mtime: Cell::new(None),
        })
        .map_err(|_| crate::Error::allocation())?;
        return Ok(ctx.alloc(Value::GzipDecoder(state)));
    }
    if matches!(name, "raw_tell" | "raw_read" | "raw_mtime" | "raw_rewind") {
        let Value::GzipDecoder(state) = ctx.heap.get(args[0]) else {
            return Err(Error::new("TypeError", "expected a gzip decoder"));
        };
        let state = state.clone();
        let reentrant = || Error::new("RuntimeError", "reentrant call inside gzip reader");
        if name == "raw_mtime" {
            let mtime = state.last_mtime.get();
            return Ok(mtime.map_or(NONE, |seconds| ctx.int(i64::from(seconds))));
        }
        if name == "raw_rewind" {
            state
                .stream
                .borrow_mut()
                .as_mut()
                .ok_or_else(reentrant)?
                .rewind();
            return Ok(NONE);
        }
        if name == "raw_tell" {
            let position = state
                .stream
                .borrow()
                .as_ref()
                .ok_or_else(reentrant)?
                .position();
            return Ok(ctx.alloc(Value::Int(crate::Int::new(position.into()))));
        }
        let size = usize::try_from(os::ssize_t(ctx, args[2])?)
            .map_err(|_| Error::new("ValueError", "negative raw gzip read size"))?;
        // One raw read can return a short prefix, even for a very large destination.
        let size = size.min(128 * 1024);
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| crate::Error::allocation())?;
        let mut decoder = state.stream.borrow_mut().take().ok_or_else(reentrant)?;
        let mut source = Source {
            ctx,
            file: args[1],
            error: None,
        };
        let count = decoder.read(
            &mut source,
            &mut bytes.spare_capacity_mut()[..size],
            &|timestamp| {
                state.last_mtime.set(Some(timestamp));
            },
        );
        *state.stream.borrow_mut() = Some(decoder);
        if let Some(error) = source.error {
            return Err(error);
        }
        let count = count.map_err(crate::objects::gzip_read_error)?;
        // SAFETY: the raw decoder initialized this prefix before returning it.
        unsafe { bytes.set_len(count) };
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    let receiver = args[0];
    if name == "open" {
        let receiver = super::posixpath::fspath(ctx, receiver)?;
        let path = os::encoded_path(ctx, receiver)?;
        let file = match FileState::gzip_reader(&path) {
            Ok(file) => file,
            Err(error) => return Err(ctx.with_errno_filename(error.into(), receiver)?),
        };
        let handle =
            crate::Shared::try_new(RefCell::new(file)).map_err(|_| crate::Error::allocation())?;
        return Ok(ctx.alloc(Value::GzipReader(handle)));
    }
    let Value::GzipReader(handle) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected a gzip reader"));
    };
    let handle = handle.clone();
    match name {
        "read" => {
            if handle.borrow().closed() {
                return Err(Error::new("ValueError", "I/O operation on closed file"));
            }
            let size = ctx.index_integer(args[1])?;
            let bytes = handle.borrow_mut().read_size(&size)?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "read1" => {
            let size = os::ssize_t(ctx, args[1])?;
            let bytes = handle.borrow_mut().read_once(usize::try_from(size).ok())?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "readline" => {
            let size = super::files::read_length(ctx, args.get(1).copied())?;
            let mut file = handle.borrow_mut();
            if file.closed() {
                return Err(Error::new("ValueError", "readline of closed file"));
            }
            let bytes = if size == 0 {
                Vec::new()
            } else {
                file.read_line_limited(usize::try_from(size).ok())?
            };
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "tell" => Ok(ctx.alloc(Value::Int(crate::Int::new(
            handle.borrow_mut().tell()?.into(),
        )))),
        "fileno" => Ok(ctx.int(i64::from(handle.borrow().fileno()?))),
        "close" => {
            handle.borrow_mut().close().map_err(super::os_error)?;
            Ok(NONE)
        }
        _ => unreachable!("registered gzip primitive"),
    }
}

/// Borrow the caller's Python stream for one raw decompression call. Its
/// exceptions must survive Rust framing helpers that operate on io::Result.
struct Source<'a> {
    ctx: &'a mut Context,
    file: Id,
    error: Option<Error>,
}

impl Source<'_> {
    fn read_from_file(&mut self, out: &mut [u8]) -> Result<usize> {
        let mark = self.ctx.heap.temporary_mark();
        let read = self.ctx.lookup_attr(self.file, "read")?;
        let size = self.ctx.int(out.len() as i64);
        let data = self.ctx.call(read, vec![size], vec![])?;
        let bytes = self.ctx.native_buffer_bytes(data)?;
        let count = bytes.len();
        if count > out.len() {
            return Err(Error::new(
                "OSError",
                "gzip input returned more bytes than requested",
            ));
        }
        out[..count].copy_from_slice(bytes);
        self.ctx.heap.release_temporaries_since(mark);
        Ok(count)
    }
}

impl Read for Source<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.read_from_file(out).map_err(|error| {
            self.error = Some(error);
            io::Error::other("gzip input read failed")
        })
    }
}
