//! Native single-member bzip2 streams used by the bundled CPython bz2 module.
pub(in crate::dynamic) mod codec;

use super::{Context, Error, Id, Result, Value, builtins, os, value};
use crate::Shared;
use std::cell::RefCell;

pub(in crate::dynamic) struct Decompressor {
    stream: codec::Decompressor,
    pub(in crate::dynamic) unused_data: Id,
}

pub(in crate::dynamic) fn field(state: &Decompressor, name: &str) -> Result<Id> {
    match name {
        "eof" => Ok(builtins::boolean(state.stream.eof())),
        "needs_input" => Ok(builtins::boolean(state.stream.needs_input())),
        "unused_data" => Ok(state.unused_data),
        _ => Err(Error::new("AttributeError", name)),
    }
}

pub(super) fn call(
    ctx: &mut Context,
    member: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (member, args) = builtins::native_codec_new(ctx, "_bz2", member, args)?;
    match member {
        "BZ2Compressor" => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "BZ2Compressor() takes no keyword arguments",
                ));
            }
            let level = match args {
                [] => 9,
                [level] => os::c_int(ctx, *level)?,
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "BZ2Compressor expected at most 1 argument, got {}",
                            args.len()
                        ),
                    ));
                }
            };
            let state = Shared::try_new(RefCell::new(codec::Compressor::new(level)?))
                .map_err(|_| crate::Error::allocation())?;
            Ok(ctx.alloc(Value::Bz2Compressor(state)))
        }
        "BZ2Decompressor" => {
            if !args.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "BZ2Decompressor() takes no positional arguments",
                ));
            }
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "BZ2Decompressor() takes no keyword arguments",
                ));
            }
            let decoder = Decompressor {
                stream: codec::Decompressor::new()?,
                unused_data: ctx.heap.bytes_from_data(Vec::new()),
            };
            let state =
                Shared::try_new(RefCell::new(decoder)).map_err(|_| crate::Error::allocation())?;
            Ok(ctx.alloc(Value::Bz2Decompressor(state)))
        }
        "BZ2Compressor.compress" => {
            let (receiver, args) = builtins::method_receiver_args(
                ctx,
                value::BZ2_COMPRESS_TYPE,
                "compress",
                args,
                kwargs,
            )?;
            let data = builtins::one_positional_arg(member, args, &[])?;
            let Value::Bz2Compressor(state) = ctx.heap.get(receiver) else {
                unreachable!()
            };
            let state = state.clone();
            let bytes = ctx.with_read_buffer(data, |ctx, view| {
                state.borrow_mut().compress(ctx.native_buffer_bytes(view)?)
            })?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "BZ2Compressor.flush" => {
            let receiver = builtins::noargs_method_receiver(
                ctx,
                value::BZ2_COMPRESS_TYPE,
                "flush",
                args,
                kwargs,
            )?;
            let Value::Bz2Compressor(state) = ctx.heap.get(receiver) else {
                unreachable!()
            };
            let bytes = state.borrow_mut().flush()?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "BZ2Decompressor.decompress" => {
            let (receiver, args) = builtins::method_receiver_args(
                ctx,
                value::BZ2_DECOMPRESS_TYPE,
                "decompress",
                args,
                &[],
            )?;
            let Value::Bz2Decompressor(state) = ctx.heap.get(receiver) else {
                unreachable!()
            };
            let state = state.clone();
            let [data, limit] = builtins::bind_builtin_arguments(
                "decompress",
                args,
                kwargs,
                ["data", "max_length"],
                1,
            )?;
            let bytes = ctx.with_read_buffer(data.expect("required argument"), |ctx, view| {
                let limit = limit
                    .map(|id| os::ssize_t(ctx, id))
                    .transpose()?
                    .unwrap_or(-1);
                let input = ctx.native_buffer_bytes(view)?;
                let mut decoder = state.borrow_mut();
                let output = decoder
                    .stream
                    .decompress(input, usize::try_from(limit).unwrap_or(usize::MAX))?;
                if decoder.stream.eof() {
                    let suffix = decoder.stream.take_unused_data();
                    if !suffix.is_empty() {
                        drop(decoder);
                        let suffix = ctx.heap.bytes_from_data(suffix);
                        state.borrow_mut().unused_data = suffix;
                    }
                }
                Ok(output)
            })?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        _ => Err(Error::new("AttributeError", member)),
    }
}
