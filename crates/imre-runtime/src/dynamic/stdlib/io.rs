//! Native base classes for binary input streams defined by runtime-loaded modules.

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

/// Install canonical class objects so subclasses inherit the same methods across imports.
pub(in super::super) fn install(ctx: &mut Context, module: Id) {
    let object = ctx.alloc(Value::Native("object".into()));
    let io = class(
        ctx,
        module,
        "IOBase",
        object,
        &[
            "closed",
            "close",
            "flush",
            "readable",
            "writable",
            "seekable",
            "__enter__",
            "__exit__",
        ],
    );
    class(ctx, module, "RawIOBase", io, &["readinto"]);
}

fn class(ctx: &mut Context, module: Id, name: &str, base: Id, methods: &[&'static str]) -> Id {
    let scope = ctx.scope("io", Some(module));
    let owner = ctx.alloc(Value::Class {
        name: format!("io.{name}"),
        scope,
        bases: vec![base],
    });
    for &name in methods {
        let method = ctx.alloc(Value::IoMethod { owner, name });
        let attr = if name == "closed" {
            ctx.alloc(Value::Property(method))
        } else {
            method
        };
        ctx.bind(scope, name, attr);
    }
    ctx.bind(module, name, owner);
    owner
}

/// Call an inherited I/O method with its bound receiver or an explicit class-level receiver.
pub(in super::super) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let arity = match name {
        "readinto" => 2,
        "__exit__" => 4,
        _ => 1,
    };
    if args.len() != arity || !kwargs.is_empty() || !ctx.instance_of(args[0], owner) {
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        ));
    }
    let receiver = args[0];
    let Value::Instance { fields, .. } = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected an IOBase instance"));
    };
    let closed = fields.contains_key("__IOBase_closed");
    match name {
        "closed" => Ok(if closed { TRUE } else { FALSE }),
        "readable" | "writable" | "seekable" => Ok(FALSE),
        "close" if closed => Ok(NONE),
        "close" => {
            let flush = ctx.lookup_attr(receiver, "flush")?;
            let result = ctx.call(flush, Vec::new(), Vec::new());
            ctx.set_attr(receiver, "__IOBase_closed", TRUE)?;
            result.map(|_| NONE)
        }
        "flush" | "__enter__" if closed => {
            Err(Error::new("ValueError", "I/O operation on closed file."))
        }
        "flush" => Ok(NONE),
        "__enter__" => Ok(receiver),
        "__exit__" => {
            let close = ctx.lookup_attr(receiver, "close")?;
            ctx.call(close, Vec::new(), Vec::new())
        }
        "readinto" => Err(Error::new("NotImplementedError", "")),
        _ => unreachable!("registered IOBase method"),
    }
}
