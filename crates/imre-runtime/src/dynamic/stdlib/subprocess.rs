//! Bind binary subprocess operations to the same child and files as typed Popen.
use super::{Context, Error, FALSE, Id, NONE, Result, Value, builtins, io, os, posixpath};
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
        executable,
        stdin,
        stdout,
        stderr,
        close_fds,
        restore_signals,
        cwd,
        env,
        bufsize,
        pipesize,
        encoding,
        errors,
        shell,
        start_new_session,
    ] = values
    else {
        return Err(Error::new("TypeError", "invalid Popen launch operands"));
    };
    let streams = [
        redirect(ctx, stdin, 0)?,
        redirect(ctx, stdout, 1)?,
        redirect(ctx, stderr, 2)?,
    ];
    if (encoding != NONE || errors != NONE) && streams.iter().any(|(mode, _)| *mode == 1) {
        // CPython opens text wrappers before the child can execute.
        io::text_options(ctx, encoding, errors, NONE)?;
    }
    let pipesize = ctx.as_int(pipesize)?;
    // A session argument can run Python during both spawn selection and fork conversion.
    let handles = (executable != NONE || start_new_session != FALSE)
        .then(|| BoxedPopen::prepare_handles(&streams, &pipesize))
        .transpose()?;
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
    let mut items = if spelling || path_like {
        vec![args]
    } else {
        ctx.items(args)?
    };
    // Truth-testing a shell override can mutate the original argument container.
    ctx.heap.reserve_temporaries(items.len())?;
    for &item in &items {
        ctx.heap.keep(item);
    }
    if shell {
        // Preserve the shell's argv[0] until argv and executable are converted separately.
        let shell_program = if executable != NONE && ctx.truth(executable)? {
            executable
        } else {
            ctx.text(BoxedPopen::default_shell())
        };
        let option = ctx.text("-c");
        items.try_reserve(2).map_err(|_| Error::new("MemoryError", ""))?;
        items.splice(..0, [shell_program, option]);
    }
    let program = if executable == NONE {
        *items
            .first()
            .ok_or_else(|| Error::new("IndexError", "list index out of range"))?
    } else {
        executable
    };
    // CPython's dirname probe can call __fspath__ before the actual executable conversion.
    let probed_directory = if handles.is_some() && BoxedPopen::probes_executable() {
        let probe = posixpath::fspath(ctx, program)?;
        match ctx.heap.get(ctx.scalar_value(probe)) {
            Value::Str(s) => s.contains('/'),
            Value::SurrogateStr(s) => s.contains(&u32::from('/')),
            Value::Bytes(s) => s.contains(&b'/'),
            _ => unreachable!("fspath returns str or bytes"),
        }
    } else {
        false
    };
    let close_fds_flag = handles.as_ref().map(|_| ctx.truth(close_fds)).transpose()?;
    let direct = match handles.as_ref().zip(close_fds_flag) {
        Some((handles, close_fds)) => Some(
            handles.uses_posix_spawn(probed_directory, cwd != NONE, close_fds, &streams)
                && !ctx.truth(start_new_session)?,
        ),
        None => None,
    };
    // Fork snapshots environ before encoding the executable; posix_spawn packs it after argv.
    let mut child_env = if direct == Some(true) {
        None
    } else {
        environment(ctx, env)?
    };
    let mut argv = Vec::new();
    argv.try_reserve_exact(items.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    // A direct launch reports the converted executable. Fork reports the original object.
    let resolved_program = posixpath::fspath(ctx, program)?;
    ctx.heap.reserve_temporaries(1)?;
    ctx.heap.keep(resolved_program);
    let program_bytes = os::c_path(ctx, resolved_program, "embedded null byte")?.into_bytes();
    let search_path = if direct == Some(false) && !program_bytes.contains(&b'/') {
        let subprocess = ctx.module("subprocess")?;
        let lookup = ctx.lookup_attr(subprocess, "_exec_search_path")?;
        let paths = ctx.call(lookup, vec![env], Vec::new())?;
        Some(path(ctx, paths)?)
    } else {
        None
    };
    let close_fds = match close_fds_flag {
        Some(value) => value,
        None => ctx.truth(close_fds)?,
    };
    let restore_signals = ctx.truth(restore_signals)?;
    let new_session = direct != Some(true) && ctx.truth(start_new_session)?;
    // On the prepared path, argv[0] is converted independently of the executable.
    let (alternate, arguments) = if handles.is_some() {
        (Some(program_bytes), items.as_slice())
    } else {
        argv.push(program_bytes);
        (None, &items[1..])
    };
    for arg in arguments {
        argv.push(path(ctx, *arg)?);
    }
    if direct == Some(true) {
        child_env = environment(ctx, env)?;
    }
    let directory = (cwd != NONE).then(|| path(ctx, cwd)).transpose()?;
    let mut warning_error = None;
    let child = BoxedPopen::launch(
        argv,
        alternate,
        direct,
        search_path,
        streams,
        handles,
        close_fds,
        restore_signals,
        directory,
        child_env,
        ctx.as_int(bufsize)?,
        pipesize,
        new_session,
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
                Some(false) => {
                    let filename = if failure.direct {
                        resolved_program
                    } else {
                        program
                    };
                    Err(ctx.with_errno_filename(error, filename)?)
                }
                None => Err(error),
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
            .wait(timeout(ctx, seconds)?, || ctx.dispatch_signals())?
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
