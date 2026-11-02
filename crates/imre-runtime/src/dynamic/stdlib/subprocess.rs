//! Bind binary subprocess operations to the same child and files as typed Popen.
use super::{Context, Error, Id, NONE, Result, Value, builtins, io, os, posixpath};
use crate::objects::BoxedPopen;

pub(super) fn function(name: &str) -> bool {
    matches!(
        name,
        "launch"
            | "pid"
            | "pipe"
            | "status"
            | "wait"
            | "signal"
            | "communicate"
            | "close_wait"
            | "finalize"
            | "cleanup"
    )
}

fn path(ctx: &mut Context, value: Id) -> Result<Vec<u8>> {
    let value = posixpath::fspath(ctx, value)?;
    Ok(os::c_path(ctx, value, "embedded null byte")?.into_bytes())
}

/// Recognize the POSIX sentinels before calling a file object's fileno().
fn redirect(ctx: &mut Context, stream: Id, index: usize) -> Result<(u32, Option<crate::Int>)> {
    if stream == NONE {
        return Ok((0, None));
    }
    let mut matches = |number| {
        let sentinel = ctx.int(number);
        ctx.checked_equal(stream, sentinel)
    };
    if matches(-1)? {
        return Ok((1, None));
    }
    if index == 2 && matches(-2)? {
        return Ok((5, None));
    }
    if matches(-3)? {
        return Ok((2, None));
    }
    let fd = if ctx.builtin_slot_accepts(stream, "int") {
        let fd = ctx.as_int(stream)?;
        if index == 1 && fd.to_i64() == Some(-2) {
            return Err(Error::new(
                "ValueError",
                "STDOUT can only be used for stderr",
            ));
        }
        fd
    } else {
        let fileno = ctx.lookup_attr(stream, "fileno")?;
        let number = ctx.call(fileno, Vec::new(), Vec::new())?;
        ctx.index_integer(number)?
    };
    // fileno() == -1 leaves the corresponding standard descriptor in place.
    Ok(if fd.to_i64() == Some(-1) {
        (0, None)
    } else {
        (6, Some(fd))
    })
}

type Environment = Vec<(Vec<u8>, Vec<u8>)>;

fn environment(ctx: &mut Context, mapping: Id) -> Result<Option<Environment>> {
    if mapping == NONE {
        return Ok(None);
    }
    let items = ctx.lookup_attr(mapping, "items")?;
    let pairs = ctx.call(items, vec![], vec![])?;
    let iter = ctx.iterator(pairs)?;
    ctx.heap.reserve_temporaries(1)?;
    ctx.heap.keep(iter);
    let mut entries = Vec::new();
    while let Some(pair) = ctx.next_item(iter)? {
        let pair = ctx.unpack_items(pair, 2, None)?;
        // A filesystem callback may mutate the mapping or discard the current pair.
        ctx.heap.reserve_temporaries(2)?;
        ctx.heap.keep(pair[0]);
        ctx.heap.keep(pair[1]);
        let key = path(ctx, pair[0])?;
        let value = path(ctx, pair[1])?;
        entries
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        entries.push((key, value));
    }
    Ok(Some(entries))
}

fn launch(ctx: &mut Context, values: &[Id]) -> Result<Id> {
    let &[
        args,
        stdin,
        stdout,
        stderr,
        close_fds,
        restore_signals,
        cwd,
        env,
        bufsize,
        pipesize,
        shell,
    ] = values
    else {
        return Err(Error::new("TypeError", "invalid Popen launch operands"));
    };
    let streams = [
        redirect(ctx, stdin, 0)?,
        redirect(ctx, stdout, 1)?,
        redirect(ctx, stderr, 2)?,
    ];
    let shell = ctx.truth(shell)?;
    let spelling = matches!(
        ctx.heap.get(ctx.scalar_value(args)),
        Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_)
    );
    let path_like = !spelling
        && (matches!(ctx.heap.get(args), Value::DirEntry(_))
            || ctx
                .slot(args, "__fspath__")
                .is_some_and(|slot| slot != NONE));
    if shell && path_like {
        return Err(Error::new(
            "TypeError",
            "path-like args is not allowed when shell is true",
        ));
    }
    let items = if spelling || path_like {
        vec![args]
    } else {
        ctx.items(args)?
    };
    if items.is_empty() && !shell {
        return Err(Error::new("IndexError", "list index out of range"));
    }
    let program = items.first().copied().unwrap_or(NONE);
    ctx.heap.reserve_temporaries(items.len())?;
    for &item in &items {
        ctx.heap.keep(item);
    }
    let env = environment(ctx, env)?;
    let mut argv = Vec::new();
    argv.try_reserve_exact(items.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    // A direct launch reports the converted executable. Fork reports the original object.
    let mut resolved_program = NONE;
    ctx.heap.reserve_temporaries(1)?;
    for item in items {
        let spelling = posixpath::fspath(ctx, item)?;
        if argv.is_empty() {
            resolved_program = spelling;
            ctx.heap.keep(spelling);
        }
        argv.push(os::c_path(ctx, spelling, "embedded null byte")?.into_bytes());
    }
    let directory = (cwd != NONE).then(|| path(ctx, cwd)).transpose()?;
    let mut warning_error = None;
    let child = BoxedPopen::launch(
        argv,
        streams,
        ctx.truth(close_fds)?,
        ctx.truth(restore_signals)?,
        directory,
        env,
        ctx.as_int(bufsize)?,
        ctx.as_int(pipesize)?,
        shell,
        &mut || {
            let result = (|| -> Result<()> {
                let warnings = ctx.module("warnings")?;
                let warn = ctx.lookup_attr(warnings, "warn")?;
                let message = ctx.text("line buffering (buffering=1) isn't supported in binary mode, the default buffer size will be used");
                let category = ctx.builtin("RuntimeWarning");
                ctx.call(warn, vec![message, category], Vec::new())?;
                Ok(())
            })();
            result.map_err(|error| {
                // Unwind the native pipe owners first, then propagate the original Python exception.
                warning_error = Some(error);
                crate::Error::new(
                    crate::objects::exceptions::Kind::RuntimeWarning,
                    "binary pipe buffering",
                )
            })
        },
    );
    if let Some(error) = warning_error {
        return Err(error);
    }
    match child {
        Ok(child) => Ok(ctx.alloc(Value::ProcessHandle(child))),
        Err(failure) => {
            let error = failure.error.into();
            match failure.directory {
                Some(true) => Err(ctx.with_errno_filename(error, cwd)?),
                Some(false) if !shell => {
                    let filename = if failure.direct {
                        resolved_program
                    } else {
                        program
                    };
                    Err(ctx.with_errno_filename(error, filename)?)
                }
                _ => Err(error),
            }
        }
    }
}

fn timeout(ctx: &Context, value: Id) -> Result<Option<f64>> {
    if value == NONE {
        return Ok(None);
    }
    ctx.real_component(ctx.scalar_value(value))?
        .map(Some)
        .ok_or_else(|| Error::new("TypeError", "expected a numeric timeout"))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "native process operations accept positional operands",
        ));
    }
    if name == "launch" {
        return launch(ctx, args);
    }
    if let ("cleanup", []) = (name, args) {
        BoxedPopen::cleanup();
        return Ok(NONE);
    }
    let Some((&handle, operands)) = args.split_first() else {
        return Err(Error::new("TypeError", "missing native process handle"));
    };
    let Value::ProcessHandle(child) = ctx.heap.get(handle) else {
        return Err(Error::new("TypeError", "expected a native process handle"));
    };
    let child = child.clone();
    match (name, operands) {
        ("pid", []) => Ok(ctx.int(i64::from(child.pid()))),
        ("finalize", []) => Ok(builtins::boolean(child.finalize())),
        ("pipe", &[index]) => {
            let index = ctx.as_integer(index)?;
            if !(0..3).contains(&index) {
                return Err(Error::new("ValueError", "invalid pipe index"));
            }
            match child.pipe(index as usize) {
                Some(file) => io::popen_pipe(ctx, file, index != 0),
                None => Ok(NONE),
            }
        }
        ("status", &[poll]) => Ok(child
            .status(ctx.truth(poll)?)?
            .map_or(NONE, |n| ctx.int(i64::from(n)))),
        ("wait", &[seconds]) => Ok(child
            .wait(timeout(ctx, seconds)?)?
            .map_or(NONE, |n| ctx.int(i64::from(n)))),
        ("signal", &[number]) => {
            let number = ctx
                .index_integer(number)?
                .to_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C int")
                })?;
            child.signal(number)?;
            Ok(NONE)
        }
        ("communicate", &[input, seconds, use_select]) => {
            let seconds = timeout(ctx, seconds)?;
            let use_select = ctx.truth(use_select)?;
            let input = if input == NONE {
                &[][..]
            } else {
                ctx.bytes_like(input)?
            };
            let exchange = child.communicate(input, seconds, use_select)?;
            let [stdout, stderr] = exchange
                .streams
                .map(|stream| stream.map_or(NONE, |data| ctx.alloc(Value::Bytes(data))));
            let budget = exchange
                .wait_budget
                .map_or(NONE, |n| ctx.alloc(Value::Float(n)));
            Ok(ctx.alloc(Value::Tuple(vec![
                stdout,
                stderr,
                builtins::boolean(exchange.timed_out),
                budget,
            ])))
        }
        ("close_wait", []) => {
            child.close_wait()?;
            Ok(NONE)
        }
        _ => Err(Error::new("TypeError", "invalid native process operation")),
    }
}
