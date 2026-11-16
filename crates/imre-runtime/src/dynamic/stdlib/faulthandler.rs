//! Handle Python arguments and ownership separately from async-signal-safe reporting.
use super::{Context, Error, Id, NONE, Result, TRUE, builtins, os_error};
use crate::fault_handler;

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) {
    for method in [
        "enable",
        "disable",
        "is_enabled",
        "dump_traceback",
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "register",
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "unregister",
    ] {
        let function = ctx.builtin(&format!("faulthandler.{method}"));
        ctx.bind(scope, method, function);
    }
}

fn destination(ctx: &mut Context, argument: Id) -> Result<(i32, Option<Id>)> {
    let file = if argument == NONE {
        let stderr = ctx
            .sys_binding("stderr")?
            .ok_or_else(|| Error::new("RuntimeError", "lost sys.stderr"))?;
        if stderr == NONE {
            return Err(Error::new("RuntimeError", "sys.stderr is None"));
        }
        stderr
    } else if let Ok(number) = ctx.as_int(argument) {
        if matches!(argument, super::FALSE | TRUE) {
            ctx.emit_builtin_warning(
                crate::objects::exceptions::Kind::RuntimeWarning,
                "bool is used as a file descriptor",
                1,
            )?;
        }
        let fd = number
            .to_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C int")
            })?;
        if fd < 0 {
            return Err(Error::new(
                "ValueError",
                "file is not a valid file descriptor",
            ));
        }
        return Ok((fd, None));
    } else {
        argument
    };
    // Either hook can remove sys.stderr, run Python and trigger a collection.
    ctx.heap.keep(file);
    let fileno = ctx.lookup_attr(file, "fileno")?;
    let result = ctx.call(fileno, Vec::new(), Vec::new())?;
    let fd = ctx
        .as_int(result)
        .ok()
        .and_then(|n| n.to_i64())
        .filter(|n| (0..i64::from(i32::MAX)).contains(n))
        .ok_or_else(|| {
            Error::new(
                "RuntimeError",
                "file.fileno() is not a valid file descriptor",
            )
        })?;
    // CPython ignores errors flushing the Python stream before writing directly to its fd.
    let _ = ctx
        .lookup_attr(file, "flush")
        .and_then(|flush| ctx.call(flush, Vec::new(), Vec::new()));
    Ok((fd as i32, Some(file)))
}

fn replace_file(ctx: &mut Context, file: Option<Id>) {
    let previous = std::mem::replace(&mut ctx.fault_file, file);
    if previous.is_some() && previous != file && ctx.heap.observes_released_roots() {
        ctx.collect_unreachable();
    }
}

fn argument(args: &[Id], kwargs: &[(String, Id)], index: usize, name: &str) -> Option<Id> {
    args.get(index).copied().or_else(|| {
        kwargs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, id)| *id)
    })
}

/// The tuple parser converts truth operands before reporting duplicate or unknown
/// names, but rejects an excessive total argument count first.
fn trace_arguments<const N: usize>(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    names: [&str; N],
) -> Result<(Id, [bool; 2])> {
    let mut flags = [true; 2];
    if args.len() + kwargs.len() <= N {
        for (i, &key) in names.iter().enumerate().skip(1) {
            if let Some(value) = argument(args, kwargs, i, key) {
                flags[i - 1] = ctx.truth(value)?;
            }
        }
    }
    let file = builtins::bind_builtin_arguments(name, args, kwargs, names, 0)?[0];
    Ok((file.unwrap_or(NONE), flags))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn check_signal(signal: i32) -> Result<()> {
    if matches!(
        signal,
        libc::SIGBUS | libc::SIGILL | libc::SIGFPE | libc::SIGABRT | libc::SIGSEGV
    ) {
        return Err(Error::new(
            "RuntimeError",
            format!("signal {signal} cannot be registered, use enable() instead"),
        ));
    }
    if !(1..crate::signals::nsig()).contains(&signal) {
        return Err(Error::new("ValueError", "signal number out of range"));
    }
    Ok(())
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "enable" => {
            let (file, [all_threads, c_stack]) =
                trace_arguments(ctx, name, args, kwargs, ["file", "all_threads", "c_stack"])?;
            let (fd, file) = destination(ctx, file)?;
            replace_file(ctx, file);
            fault_handler::enable(fd, all_threads, c_stack).map_err(os_error)?;
            ctx.publish_fault_frames();
            Ok(NONE)
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "register" => {
            let names = ["signum", "file", "all_threads", "chain"];
            // PyArg parses known operands in signature order before diagnosing a duplicate
            // or unknown keyword. Conversion hooks can raise or change the destination.
            let converted = if args.len() + kwargs.len() <= names.len()
                && let Some(number) = argument(args, kwargs, 0, names[0])
            {
                let signal = ctx.index_c_int(number)?;
                let all = argument(args, kwargs, 2, names[2])
                    .map(|id| ctx.truth(id))
                    .transpose()?
                    .unwrap_or(true);
                let chain = argument(args, kwargs, 3, names[3])
                    .map(|id| ctx.truth(id))
                    .transpose()?
                    .unwrap_or(false);
                Some((signal, all, chain))
            } else {
                None
            };
            let [_, file, _, _] =
                builtins::bind_builtin_arguments("register", args, kwargs, names, 1)?;
            let (signal, all, chain) = converted.expect("successful binding has a signal");
            check_signal(signal)?;
            let (fd, file) = destination(ctx, file.unwrap_or(NONE))?;
            fault_handler::register(signal, fd, all, chain).map_err(os_error)?;
            let old = match file {
                Some(id) => ctx.fault_user_files.insert(signal, id),
                None => ctx.fault_user_files.remove(&signal),
            };
            if old.is_some() && old != file && ctx.heap.observes_released_roots() {
                ctx.collect_unreachable();
            }
            ctx.publish_fault_frames();
            Ok(NONE)
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "unregister" => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "unregister() takes no keyword arguments",
                ));
            }
            let [number] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "unregister() takes exactly 1 argument ({} given)",
                        args.len()
                    ),
                ));
            };
            let signal = ctx.index_c_int(*number)?;
            check_signal(signal)?;
            let changed = fault_handler::unregister(signal).map_err(os_error)?;
            if ctx.fault_user_files.remove(&signal).is_some() && ctx.heap.observes_released_roots()
            {
                ctx.collect_unreachable();
            }
            Ok(builtins::boolean(changed))
        }
        "dump_traceback" => {
            let (file, [all_threads, _]) =
                trace_arguments(ctx, name, args, kwargs, ["file", "all_threads"])?;
            let (fd, _) = destination(ctx, file)?;
            ctx.publish_fault_frames();
            fault_handler::dump_traceback(fd, all_threads);
            Ok(NONE)
        }
        "is_enabled" | "disable" => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!("faulthandler.{name}() takes no keyword arguments"),
                ));
            }
            if !args.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "faulthandler.{name}() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            if name == "is_enabled" {
                return Ok(builtins::boolean(fault_handler::is_enabled()));
            }
            let enabled = fault_handler::disable();
            replace_file(ctx, None);
            Ok(builtins::boolean(enabled))
        }
        _ => unreachable!("registered faulthandler method"),
    }
}
