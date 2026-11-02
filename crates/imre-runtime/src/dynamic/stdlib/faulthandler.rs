//! Handle Python arguments and ownership separately from async-signal-safe reporting.
use super::{Context, Error, Id, NONE, Result, TRUE, builtins, os_error};
use crate::fault_handler;

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) {
    for method in ["enable", "disable", "is_enabled", "dump_traceback"] {
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
            let value = args
                .get(i)
                .or_else(|| kwargs.iter().find(|(k, _)| k == key).map(|(_, v)| v));
            if let Some(&value) = value {
                flags[i - 1] = ctx.truth(value)?;
            }
        }
    }
    let file = builtins::bind_builtin_arguments(name, args, kwargs, names, 0)?[0];
    Ok((file.unwrap_or(NONE), flags))
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
