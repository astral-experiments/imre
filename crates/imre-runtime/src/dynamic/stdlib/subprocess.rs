//! Bind binary subprocess operations to the same child and files as typed Popen.
use super::{Context, Error, FALSE, Id, NONE, Result, Value, builtins, io, os, posixpath};
use crate::objects::{BoxedPopen, BoxedPreparation};

pub(super) fn function(name: &str) -> bool {
    matches!(
        name,
        "prepare"
            | "release"
            | "launch"
            | "pid"
            | "pipe"
            | "poll"
            | "wait"
            | "signal"
            | "communicate"
            | "close_wait"
            | "cleanup"
    )
}

fn path(ctx: &mut Context, value: Id, defer_nul: bool) -> Result<Vec<u8>> {
    let value = posixpath::fspath(ctx, value)?;
    if defer_nul {
        os::encoded_path(ctx, value)
    } else {
        Ok(os::c_path(ctx, value, "embedded null byte")?.into_bytes())
    }
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

fn environment(ctx: &mut Context, mapping: Id, fork: bool) -> Result<Option<Environment>> {
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
        let key = path(ctx, pair[0], fork)?;
        if fork && key.contains(&b'=') {
            return Err(Error::new(
                "ValueError",
                "illegal environment variable name",
            ));
        }
        let value = path(ctx, pair[1], fork)?;
        entries
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        entries.push((key, value));
    }
    Ok(Some(entries))
}

fn prepare(ctx: &mut Context, values: &[Id]) -> Result<Id> {
    let &[stdin, stdout, stderr, bufsize, pipesize] = values else {
        return Err(Error::new(
            "TypeError",
            "invalid Popen preparation operands",
        ));
    };
    let mut streams = [
        redirect(ctx, stdin, 0)?,
        redirect(ctx, stdout, 1)?,
        redirect(ctx, stderr, 2)?,
    ];
    let pipe_count = streams.iter().filter(|(mode, _)| *mode == 1).count();
    let bufsize = ctx.as_int(bufsize)?;
    let pipesize = ctx.as_int(pipesize)?;
    let warn_buffering = pipe_count != 0 && bufsize.as_small() == Some(1);
    let mut handles = BoxedPopen::prepare_handles(&streams, &pipesize)?;
    if streams[1].0 == 0 && streams[2].0 == 5 {
        let sys = ctx.module("sys")?;
        let stdout = ctx.lookup_attr(sys, "__stdout__")?;
        let fileno = ctx.lookup_attr(stdout, "fileno")?;
        let fd = ctx.call(fileno, Vec::new(), Vec::new())?;
        handles.inherit_stderr(&mut streams[2], ctx.index_integer(fd)?);
    }
    let (prepared, fds) = BoxedPreparation::new(streams, handles, bufsize, pipesize)?;
    if warn_buffering {
        for _ in 0..pipe_count {
            let warnings = ctx.module("warnings")?;
            let warn = ctx.lookup_attr(warnings, "warn")?;
            let message = ctx.text("line buffering (buffering=1) isn't supported in binary mode, the default buffer size will be used");
            let category = ctx.builtin("RuntimeWarning");
            ctx.call(warn, vec![message, category], Vec::new())?;
        }
    }
    let fds = fds.map(|fd| ctx.alloc(Value::Int(fd)));
    let fds = ctx.alloc(Value::Tuple(fds.into()));
    let owner = ctx.alloc(Value::ProcessPreparation(std::rc::Rc::new(
        std::cell::RefCell::new(prepared),
    )));
    Ok(ctx.alloc(Value::Tuple(vec![owner, fds])))
}

fn extra_group_ids(ctx: &mut Context, gids: Id) -> Result<Option<Vec<libc::gid_t>>> {
    if gids == NONE {
        return Ok(None);
    }
    if !ctx.builtin_slot_accepts(gids, "list") {
        return Err(Error::new("TypeError", "setgroups argument must be a list"));
    }
    let count = ctx.sequence_len(gids)?;
    unsafe extern "C" {
        fn imre_platform_max_groups() -> usize;
    }
    // SAFETY: this function returns the limit from the target's C headers.
    if count > unsafe { imre_platform_max_groups() } {
        return Err(Error::new("ValueError", "too many extra_groups"));
    }
    let mut groups = Vec::new();
    groups
        .try_reserve_exact(count)
        .map_err(|_| Error::new("MemoryError", ""))?;
    for i in 0..count {
        let index = ctx.int(i as i64);
        let value = ctx.get_item(gids, index)?;
        if !ctx.builtin_slot_accepts(value, "int") {
            return Err(Error::new("TypeError", "extra_groups must be integers"));
        }
        let gid = os::owner_id(ctx, value, true)
            .map_err(|_| Error::new("ValueError", "invalid group id"))?;
        groups.push(gid as libc::gid_t);
    }
    Ok(Some(groups))
}

fn launch(ctx: &mut Context, values: &[Id]) -> Result<Id> {
    let &[
        owner,
        args,
        executable,
        close_fds,
        restore_signals,
        cwd,
        env,
        shell,
        start_new_session,
        process_group,
        umask,
        pass_fds,
        gids,
    ] = values
    else {
        return Err(Error::new("TypeError", "invalid Popen launch operands"));
    };
    let Value::ProcessPreparation(prepared) = ctx.heap.get(owner) else {
        return Err(Error::new(
            "TypeError",
            "expected prepared subprocess pipes",
        ));
    };
    let launch = prepared
        .borrow_mut()
        .launch
        .take()
        .ok_or_else(|| Error::new("RuntimeError", "subprocess pipes have already been used"))?;
    let (streams, handles, bufsize, pipesize) = (
        launch.streams,
        launch.handles,
        launch.bufsize,
        launch.pipesize,
    );
    let default_fds = matches!(ctx.heap.get(pass_fds), Value::Tuple(fds) if fds.is_empty());
    let minus_one = |id| matches!(ctx.heap.get(id), Value::Int(n) if n.as_small() == Some(-1));
    let default_group = process_group == NONE || minus_one(process_group);
    let default_umask = minus_one(umask);
    // Retain the existing conversion order on the native default-argument path.
    let probe = executable != NONE
        || start_new_session != FALSE
        || !default_group
        || gids != NONE
        || !default_umask
        || !default_fds
        || (streams.iter().any(|(mode, _)| *mode == 1) && bufsize.as_small() != Some(-1));
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
        items
            .try_reserve(2)
            .map_err(|_| Error::new("MemoryError", ""))?;
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
    let probed_directory = if probe && BoxedPopen::probes_executable() {
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
    let mut direct = match probe.then_some(&handles) {
        Some(handles) if probed_directory => {
            let close_fds = ctx.truth(close_fds)?;
            // CPython tests pass_fds before cwd, redirected handles and session settings.
            let passing_fds = !default_fds
                && (!close_fds || BoxedPopen::spawn_closes_fds())
                && ctx.truth(pass_fds)?;
            Some(
                !passing_fds
                    && handles.uses_posix_spawn(probed_directory, cwd != NONE, close_fds, &streams)
                    && !ctx.truth(start_new_session)?,
            )
        }
        Some(_) => Some(false),
        None => None,
    };
    if direct == Some(true) && !default_group {
        let inherited = ctx.int(-1);
        direct = Some(ctx.checked_equal(process_group, inherited)?);
    }
    if direct == Some(true) && gids != NONE {
        direct = Some(false);
    }
    if direct == Some(true) && !default_umask {
        let zero = ctx.int(0);
        let negative = ctx.binary("<", umask, zero)?;
        direct = Some(ctx.truth(negative)?);
    }
    // Fork snapshots environ before encoding the executable; posix_spawn packs it after argv.
    let mut child_env = if direct == Some(true) {
        None
    } else {
        environment(ctx, env, direct == Some(false))?
    };
    let mut argv = Vec::new();
    argv.try_reserve_exact(items.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    // A direct launch reports the converted executable. Fork reports the original object.
    let resolved_program = posixpath::fspath(ctx, program)?;
    ctx.heap.reserve_temporaries(1)?;
    ctx.heap.keep(resolved_program);
    // Fork validates NULs in C after converting its integer arguments.
    let program_bytes = if direct == Some(false) {
        os::encoded_path(ctx, resolved_program)?
    } else {
        os::c_path(ctx, resolved_program, "embedded null byte")?.into_bytes()
    };
    let search_path = if direct == Some(false) && !program_bytes.contains(&b'/') {
        let subprocess = ctx.module("subprocess")?;
        let lookup = ctx.lookup_attr(subprocess, "_exec_search_path")?;
        let paths = ctx.call(lookup, vec![env], Vec::new())?;
        Some(path(ctx, paths, true)?)
    } else {
        None
    };
    let normalized = if !default_fds && direct != Some(true) {
        let subprocess = ctx.module("subprocess")?;
        let normalize = ctx.lookup_attr(subprocess, "_fork_fds")?;
        let fds = ctx.call(normalize, vec![pass_fds], Vec::new())?;
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(fds);
        Some(fds)
    } else {
        None
    };
    let (close_fds, restore_signals) = if direct == Some(true) {
        // _posix_spawn builds signal defaults before its closefrom file action.
        let restore = ctx.truth(restore_signals)?;
        (ctx.truth(close_fds)?, restore)
    } else {
        (ctx.truth(close_fds)?, ctx.truth(restore_signals)?)
    };
    let new_session = direct != Some(true) && ctx.truth(start_new_session)?;
    let group = if default_group || direct == Some(true) {
        -1
    } else {
        os::c_int(ctx, process_group)?
    };
    let umask = if default_umask || direct == Some(true) {
        -1
    } else {
        os::c_int(ctx, umask)?
    };
    // fork_exec parses process_group and umask before checking the normalized descriptors.
    let mut keep = Vec::new();
    if let Some(fds) = normalized {
        for fd in ctx.items(fds)? {
            BoxedPopen::push_pass_fd(&mut keep, &ctx.as_int(fd)?)?;
        }
    }
    // A supplementary-group launch must validate executable candidates before argv and gids.
    if gids != NONE
        && (program_bytes.contains(&0)
            || search_path.as_ref().is_some_and(|path| path.contains(&0)))
    {
        return Err(Error::new("ValueError", "embedded null byte"));
    }
    // On the prepared path, argv[0] is converted independently of the executable.
    let (alternate, arguments) = if probe {
        (Some(program_bytes), items.as_slice())
    } else {
        argv.push(program_bytes);
        (None, &items[1..])
    };
    for arg in arguments {
        argv.push(path(ctx, *arg, false)?);
    }
    if direct == Some(true) {
        child_env = environment(ctx, env, false)?;
    } else if gids != NONE
        && child_env.as_ref().is_some_and(|entries| {
            entries
                .iter()
                .any(|(key, value)| key.contains(&0) || value.contains(&0))
        })
    {
        // The fork environment was encoded earlier but is checked after argv, before cwd.
        return Err(Error::new("ValueError", "embedded null byte"));
    }
    let directory = (cwd != NONE).then(|| path(ctx, cwd, false)).transpose()?;
    let groups = extra_group_ids(ctx, gids)?;
    let child = BoxedPopen::launch(
        argv,
        alternate,
        direct,
        search_path,
        streams,
        Some(handles),
        close_fds,
        restore_signals,
        directory,
        child_env,
        bufsize,
        pipesize,
        new_session,
        group,
        umask,
        keep,
        groups,
    );
    match child {
        Ok(child) => Ok(ctx.alloc(Value::ProcessHandle {
            child,
            reported: false,
        })),
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

// Python owns the mutable returncode. Keep the physical reap in ChildControl so
// a later assignment cannot authorize signaling a PID that has been released.
fn report_status(ctx: &mut Context, handle: Id, status: Option<i32>) -> Id {
    let Some(code) = status else { return NONE };
    let Value::ProcessHandle { reported, .. } = ctx.heap.get_mut(handle) else {
        unreachable!("validated process handle")
    };
    // A returning signal handler can already have waited and reset returncode.
    let code = if std::mem::replace(reported, true) {
        0
    } else {
        code
    };
    ctx.int(i64::from(code))
}

enum StopWait {
    Assigned,
    Raised(Error),
}

impl From<crate::Error> for StopWait {
    fn from(error: crate::Error) -> Self {
        Self::Raised(error.into())
    }
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
    if name == "prepare" {
        return prepare(ctx, args);
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
    if let Value::ProcessPreparation(prepared) = ctx.heap.get(handle) {
        let prepared = prepared.clone();
        return match (name, operands) {
            ("release", &[failed]) => {
                let failed = ctx.truth(failed)?;
                prepared.borrow_mut().release(failed);
                Ok(NONE)
            }
            ("pipe", &[index]) => {
                let index = ctx.as_integer(index)?;
                if !(0..3).contains(&index) {
                    return Err(Error::new("ValueError", "invalid pipe index"));
                }
                let file = prepared.borrow().pipe(index as usize);
                match file {
                    Some(file) => io::popen_pipe(ctx, file, index != 0),
                    None => Ok(NONE),
                }
            }
            _ => Err(Error::new(
                "TypeError",
                "invalid operation on prepared pipes",
            )),
        };
    }
    let Value::ProcessHandle { child, reported } = ctx.heap.get(handle) else {
        return Err(Error::new("TypeError", "expected a native process handle"));
    };
    let child = child.clone();
    let reported = *reported;
    match (name, operands) {
        ("pid", []) => Ok(ctx.int(i64::from(child.pid()))),
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
        // Python asks again only after its public status was cleared. A real
        // second wait would get ECHILD, which Popen reports as zero.
        ("poll", []) | ("wait", [_, _]) if reported => Ok(ctx.int(0)),
        ("poll", []) => Ok(report_status(ctx, handle, child.poll()?)),
        ("wait", &[seconds, owner]) => {
            let seconds = timeout(ctx, seconds)?;
            match child.wait(seconds, || {
                ctx.dispatch_signals().map_err(StopWait::Raised)?;
                // A timed wait checks public status between polls. A blocking
                // waitpid resumes after a returning handler and reports its reap.
                if seconds.is_some() {
                    let status = ctx
                        .lookup_attr(owner, "returncode")
                        .map_err(StopWait::Raised)?;
                    if status != NONE {
                        return Err(StopWait::Assigned);
                    }
                }
                Ok(())
            }) {
                Ok(code) => Ok(report_status(ctx, handle, code)),
                Err(StopWait::Assigned) => Ok(NONE),
                Err(StopWait::Raised(error)) => Err(error),
            }
        }
        ("signal", &[number]) => {
            // os.kill converts its signal to Py_ssize_t before narrowing for POSIX.
            let number = os::ssize_t(ctx, number)?;
            child.signal(number as libc::c_int)?;
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
            let pending = child.begin_communication(input, seconds, use_select)?;
            let exchange = child.finish_communication(pending, || ctx.dispatch_signals())?;
            let [stdout, stderr] = exchange
                .streams
                .map(|stream| stream.map_or(NONE, |data| ctx.alloc(Value::Bytes(data))));
            Ok(ctx.alloc(Value::Tuple(vec![
                stdout,
                stderr,
                builtins::boolean(exchange.timed_out),
            ])))
        }
        ("close_wait", []) => {
            child.close_wait()?;
            Ok(NONE)
        }
        _ => Err(Error::new("TypeError", "invalid native process operation")),
    }
}
