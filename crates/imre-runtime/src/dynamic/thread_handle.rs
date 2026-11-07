//! Public _thread handle descriptors share the worker's native lifecycle.
use super::{Context, Error, Id, NONE, Result, Value, builtins, stdlib};
use std::time::Duration;

pub(super) const CLASS: &str = "_thread._ThreadHandle";
pub(super) const METHODS: &[&str] = &["__repr__", "join", "_set_done", "is_done"];

pub(super) fn install(ctx: &mut Context, class: Id) {
    let ident = ctx.builtin("_thread._ThreadHandle._get_ident");
    let descriptor = ctx.alloc(Value::GetSetDescriptor {
        owner: CLASS,
        name: "ident",
        get: ident,
        set: None,
    });
    let constructor = ctx.builtin("_thread._ThreadHandle.__new__");
    let mut entries = vec![(ctx.heap.code_name("__new__"), constructor)];
    for &name in METHODS {
        let function = ctx.builtin(&format!("{CLASS}.{name}"));
        entries.push((ctx.heap.code_name(name), function));
    }
    entries.extend([
        (ctx.heap.code_name("ident"), descriptor),
        (ctx.heap.code_name("__doc__"), NONE),
        (
            ctx.heap.code_name("__module__"),
            ctx.heap.code_name("_thread"),
        ),
    ]);
    let namespace = ctx.dict(entries);
    ctx.native_type_dicts.insert(class, namespace);
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__repr__" {
        let Some((&receiver, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("descriptor '__repr__' of '{CLASS}' object needs an argument"),
            ));
        };
        if !matches!(ctx.heap.get(receiver), Value::ThreadHandle(_)) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '__repr__' requires a '{CLASS}' object but received a '{}'",
                    ctx.type_name(receiver)
                ),
            ));
        }
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "wrapper __repr__() takes no keyword arguments",
            ));
        }
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("expected 0 arguments, got {}", rest.len()),
            ));
        }
        return Ok(ctx.text(ctx.repr(receiver)));
    }
    let method = if name == "_get_ident" { "ident" } else { name };
    let (receiver, args) = builtins::method_receiver_args(ctx, CLASS, method, args, kwargs)?;
    let Value::ThreadHandle(handle) = ctx.heap.get(receiver) else {
        unreachable!("validated thread handle");
    };
    let handle = handle.clone();
    if name == "join" {
        let timeout = match args {
            [] | [NONE] => None,
            [seconds] => u64::try_from(stdlib::time::timeout_ns(ctx, *seconds)?)
                .ok()
                .map(Duration::from_nanos),
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("join() takes at most 1 argument ({} given)", args.len()),
                ));
            }
        };
        handle.join_public(ctx, timeout)?;
        ctx.reap_thread_operands();
        return Ok(NONE);
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "_ThreadHandle.{name}() takes no arguments ({} given)",
                args.len()
            ),
        ));
    }
    match name {
        "_get_ident" => Ok(ctx.int(handle.ident() as i64)),
        "is_done" => {
            let done = handle.is_done();
            if done {
                ctx.reap_thread_operands();
            }
            Ok(builtins::boolean(done))
        }
        "_set_done" => {
            handle.set_done()?;
            Ok(NONE)
        }
        _ => unreachable!("registered thread handle method"),
    }
}
