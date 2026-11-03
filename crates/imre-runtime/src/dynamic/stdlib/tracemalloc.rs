//! Allocation tracebacks for objects owned by the boxed interpreter.

use super::{Context, Error, Id, NONE, Result, Value};
use crate::dynamic::{FALSE, TRUE};

const METHODS: &[(&str, &str, &str)] = &[
    // Argument Clinic signatures and docs from CPython 3.14.5.
    (
        "start",
        "($module, nframe=1, /)",
        "Start tracing Python memory allocations.\n\nAlso set the maximum number of frames stored in the traceback of a\ntrace to nframe.",
    ),
    (
        "stop",
        "($module, /)",
        "Stop tracing Python memory allocations.\n\nAlso clear traces of memory blocks allocated by Python.",
    ),
    (
        "is_tracing",
        "($module, /)",
        "Return True if the tracemalloc module is tracing Python memory allocations.",
    ),
    (
        "clear_traces",
        "($module, /)",
        "Clear traces of memory blocks allocated by Python.",
    ),
    (
        "get_traceback_limit",
        "($module, /)",
        "Get the maximum number of frames stored in the traceback of a trace.\n\nBy default, a trace of an allocated memory block only stores\nthe most recent frame: the limit is 1.",
    ),
    (
        "_get_object_traceback",
        "($module, obj, /)",
        "Get the traceback where the Python object obj was allocated.\n\nReturn a tuple of (filename: str, lineno: int) tuples.\nReturn None if the tracemalloc module is disabled or did not\ntrace the allocation of the object.",
    ),
];

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) {
    for &(name, _, _) in METHODS {
        let callable = ctx.builtin(&format!("_tracemalloc.{name}"));
        let method = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.bind(scope, name, method);
    }
}

pub(in crate::dynamic) fn method_metadata(
    name: &str,
) -> Option<(&'static str, Option<&'static str>)> {
    METHODS
        .iter()
        .find(|m| m.0 == name)
        .map(|m| (m.1, Some(m.2)))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((_, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "expected a _tracemalloc module receiver",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("_tracemalloc.{name}() takes no keyword arguments"),
        ));
    }
    match name {
        "start" => {
            let depth = match args {
                [] => 1,
                &[arg] => ctx
                    .index_integer(arg)?
                    .to_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| {
                        Error::new("OverflowError", "Python int too large to convert to C int")
                    })?,
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!("start expected at most 1 argument, got {}", args.len()),
                    ));
                }
            };
            if !(1..=65535).contains(&depth) {
                return Err(Error::new(
                    "ValueError",
                    "the number of frames must be in range [1; 65535]",
                ));
            }
            ctx.heap.allocation_traces.start(depth as usize);
        }
        "_get_object_traceback" => {
            let &[object] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "_tracemalloc.{name}() takes exactly one argument ({} given)",
                        args.len()
                    ),
                ));
            };
            let Some(trace) = ctx.heap.allocation_traces.get(object) else {
                return Ok(NONE);
            };
            let mut frames = Vec::with_capacity(trace.len());
            for (file, line) in trace.iter() {
                let filename = ctx.text(file.as_ref());
                let lineno = ctx.int(i64::from(*line));
                frames.push(ctx.heap.packed_tuple(vec![filename, lineno], false));
            }
            return Ok(ctx.heap.packed_tuple(frames, false));
        }
        _ if !args.is_empty() => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "_tracemalloc.{name}() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        "stop" => ctx.heap.allocation_traces.stop(),
        "clear_traces" => ctx.heap.allocation_traces.clear(),
        "is_tracing" => {
            return Ok(if ctx.heap.allocation_traces.is_tracing() {
                TRUE
            } else {
                FALSE
            });
        }
        "get_traceback_limit" => return Ok(ctx.int(ctx.heap.allocation_traces.limit() as i64)),
        _ => unreachable!("unregistered _tracemalloc method"),
    }
    Ok(NONE)
}
