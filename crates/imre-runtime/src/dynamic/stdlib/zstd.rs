//! Incremental Zstandard streams for compression.zstd.
use super::{Context, Error, Id, NONE, Result, Value, builtins, os, value};
use crate::zstd_streams as streams;
use num_traits::ToPrimitive;
use std::sync::Mutex;
use triomphe::Arc;

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) {
    for name in ["ZstdCompressor", "ZstdDecompressor", "ZstdError"] {
        let class = ctx.builtin(&format!("compression.zstd.{name}"));
        ctx.bind(scope, name, class);
    }
    for (name, number) in [
        ("ZSTD_CLEVEL_DEFAULT", i64::from(zstd_safe::CLEVEL_DEFAULT)),
        (
            "zstd_version_number",
            i64::from(zstd_safe::version_number()),
        ),
        ("ZSTD_DStreamOutSize", zstd_safe::DCtx::out_size() as i64),
    ] {
        let id = ctx.int(number);
        ctx.bind(scope, name, id);
    }
    let version = ctx.text(zstd_safe::version_string());
    ctx.bind(scope, "zstd_version", version);
}

#[derive(Clone)]
pub(in crate::dynamic) struct Decompressor {
    stream: Arc<Mutex<streams::Decompressor>>,
    // Keep the Python reference in the heap object, outside the provider lock.
    pub(in crate::dynamic) unused_data: Id,
}

impl Decompressor {
    pub(in crate::dynamic) fn field(&self, name: &str) -> Id {
        match name {
            "eof" => builtins::boolean(self.stream.lock().unwrap().eof),
            "needs_input" => builtins::boolean(self.stream.lock().unwrap().needs_input),
            "unused_data" => self.unused_data,
            _ => unreachable!("unknown decompressor member"),
        }
    }
}

fn options(ctx: &Context, kind: &str, options: Option<Id>) -> Result<()> {
    if let Some(id) = options.filter(|&id| id != NONE) {
        let entries = ctx.dict_entries(ctx.scalar_value(id)).ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "{kind}() argument 'options' must be dict, not {}",
                    ctx.type_name(id)
                ),
            )
        })?;
        if !entries.is_empty() {
            return Err(Error::new(
                "NotImplementedError",
                "advanced Zstandard parameters are not supported",
            ));
        }
    }
    Ok(())
}

fn dictionary(id: Option<Id>) -> Result<()> {
    if id.is_some_and(|id| id != NONE) {
        return Err(Error::new(
            "TypeError",
            "zstd_dict argument should be a ZstdDict object.",
        ));
    }
    Ok(())
}

pub(super) fn call(
    ctx: &mut Context,
    member: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (member, args) = builtins::native_codec_new(ctx, "compression.zstd", member, args)?;
    match member {
        "ZstdCompressor" => {
            let [level, params, dict] = builtins::bind_method_arguments(
                member,
                args,
                kwargs,
                ["level", "options", "zstd_dict"],
                3,
            )?;
            let level = level.filter(|&id| id != NONE);
            if level.is_some() && params.is_some_and(|id| id != NONE) {
                return Err(Error::new(
                    "TypeError",
                    "Only one of level or options should be used.",
                ));
            }
            let level = level
                .map(|id| {
                    ctx.as_int(id)
                        .map_err(|_| {
                            Error::new("TypeError", "invalid type for level, expected int")
                        })?
                        .to_i64()
                        .and_then(|n| i32::try_from(n).ok())
                        .ok_or_else(|| {
                            Error::new(
                                "ValueError",
                                format!(
                                    "illegal compression level; the valid range is [{}, {}]",
                                    zstd_safe::min_c_level(),
                                    zstd_safe::max_c_level()
                                ),
                            )
                        })
                })
                .transpose()?;
            options(ctx, member, params)?;
            dictionary(dict)?;
            let state = Arc::try_new(Mutex::new(streams::Compressor::new(level)?))
                .map_err(|_| crate::Error::allocation())?;
            Ok(ctx.alloc(Value::ZstdCompressor(state)))
        }
        "ZstdDecompressor" => {
            let [dict, params] =
                builtins::bind_method_arguments(member, args, kwargs, ["zstd_dict", "options"], 2)?;
            dictionary(dict)?;
            options(ctx, member, params)?;
            let stream = Arc::try_new(Mutex::new(streams::Decompressor::new()?))
                .map_err(|_| crate::Error::allocation())?;
            let unused_data = ctx.heap.bytes_from_data(Vec::new());
            Ok(ctx.alloc(Value::ZstdDecompressor(Decompressor {
                stream,
                unused_data,
            })))
        }
        "ZstdCompressor.compress"
        | "ZstdCompressor.flush"
        | "ZstdCompressor.set_pledged_input_size" => {
            let (_, method) = member.split_once('.').unwrap();
            let (receiver, args) =
                builtins::method_receiver_args(ctx, value::ZSTD_COMPRESS_TYPE, method, args, &[])?;
            let Value::ZstdCompressor(state) = ctx.heap.get(receiver) else {
                unreachable!()
            };
            let state = state.clone();
            let bytes = match method {
                "compress" => {
                    let [data, mode] = builtins::bind_builtin_arguments(
                        method,
                        args,
                        kwargs,
                        ["data", "mode"],
                        1,
                    )?;
                    ctx.with_read_buffer(data.unwrap(), |ctx, view| {
                        let mode = mode.map(|id| os::c_int(ctx, id)).transpose()?.unwrap_or(0);
                        let input = ctx.native_buffer_bytes(view)?;
                        state
                            .lock()
                            .unwrap()
                            .compress(input, mode)
                            .map_err(Error::from)
                    })?
                }
                "flush" => {
                    let [mode] =
                        builtins::bind_method_arguments(method, args, kwargs, ["mode"], 1)?;
                    let mode = mode.map(|id| os::c_int(ctx, id)).transpose()?.unwrap_or(2);
                    if !matches!(mode, 1 | 2) {
                        return Err(Error::new(
                            "ValueError",
                            "mode argument wrong value, it should be ZstdCompressor.FLUSH_FRAME or ZstdCompressor.FLUSH_BLOCK.",
                        ));
                    }
                    state.lock().unwrap().compress(&[], mode)?
                }
                _ => {
                    let size = builtins::one_positional_arg(member, args, kwargs)?;
                    let size =
                        if size == NONE {
                            None
                        } else {
                            let n = ctx
                                .as_int(size)
                                .map_err(|_| Error::new("TypeError", "an integer is required"))?;
                            Some(n.big().to_u64().filter(|&n| n < u64::MAX - 1).ok_or_else(
                                || {
                                    Error::new(
                                        "ValueError",
                                        format!(
                                            "size argument should be a positive int less than {}",
                                            u64::MAX - 1
                                        ),
                                    )
                                },
                            )?)
                        };
                    state.lock().unwrap().set_pledged_input_size(size)?;
                    return Ok(NONE);
                }
            };
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "ZstdDecompressor.decompress" => {
            let (receiver, args) = builtins::method_receiver_args(
                ctx,
                value::ZSTD_DECOMPRESS_TYPE,
                "decompress",
                args,
                &[],
            )?;
            let Value::ZstdDecompressor(state) = ctx.heap.get(receiver) else {
                unreachable!()
            };
            let stream = state.stream.clone();
            let [data, limit] = builtins::bind_builtin_arguments(
                "decompress",
                args,
                kwargs,
                ["data", "max_length"],
                1,
            )?;
            let bytes = ctx.with_read_buffer(data.unwrap(), |ctx, view| {
                let max_length = limit
                    .map(|id| os::ssize_t(ctx, id))
                    .transpose()?
                    .unwrap_or(-1);
                let input = ctx.native_buffer_bytes(view)?;
                let mut decoder = stream.lock().unwrap();
                let out =
                    decoder.decompress(input, usize::try_from(max_length).unwrap_or(usize::MAX))?;
                if decoder.eof {
                    let suffix = decoder.take_unused_data();
                    drop(decoder);
                    if !suffix.is_empty() {
                        let id = ctx.heap.bytes_from_data(suffix);
                        let Value::ZstdDecompressor(state) = ctx.heap.get_mut(receiver) else {
                            unreachable!()
                        };
                        state.unused_data = id;
                    }
                }
                Ok(out)
            })?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        _ => Err(Error::new("AttributeError", member)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_streams_keep_their_frame_across_a_thread_handoff() {
        let mut ctx = crate::dynamic::ffi::new_context(
            serde_json::json!({"paths": []}),
            std::iter::empty::<String>(),
        );
        let c = call(&mut ctx, "ZstdCompressor", &[], &[]).unwrap();
        let d = call(&mut ctx, "ZstdDecompressor", &[], &[]).unwrap();
        let Value::ZstdCompressor(encoder) = ctx.heap.get(c) else {
            unreachable!()
        };
        let Value::ZstdDecompressor(decoder) = ctx.heap.get(d) else {
            unreachable!()
        };
        let encoder = encoder.clone();
        let decoder = decoder.stream.clone();
        let block = encoder
            .lock()
            .unwrap()
            .compress(b"first ", 1)
            .map_err(Error::from)
            .unwrap();
        assert_eq!(
            decoder
                .lock()
                .unwrap()
                .decompress(&block, 3)
                .map_err(Error::from)
                .unwrap(),
            b"fir"
        );

        drop(ctx);
        std::thread::spawn(move || {
            let mut frame = encoder
                .lock()
                .unwrap()
                .compress(b"second", 2)
                .map_err(Error::from)
                .unwrap();
            frame.extend_from_slice(b"tail");
            let mut stream = decoder.lock().unwrap();
            assert_eq!(
                stream
                    .decompress(&frame, usize::MAX)
                    .map_err(Error::from)
                    .unwrap(),
                b"st second"
            );
            assert!(stream.eof);
            assert_eq!(stream.take_unused_data(), b"tail");
        })
        .join()
        .unwrap();
    }
}
