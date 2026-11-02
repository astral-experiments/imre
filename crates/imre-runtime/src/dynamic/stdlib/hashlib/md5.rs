//! The portable MD5 factory and its type belong to one module initialization.
use super::{Context, Error, Id, NONE, Result, Value};

pub(in crate::dynamic) const CLASS: &str = "_md5.md5";

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let fields = ctx.scope("_md5", Some(scope));
    let module_name = ctx.text("_md5");
    let short_name = ctx.text("md5");
    ctx.bind(fields, "__module__", module_name);
    ctx.bind(fields, "__qualname__", short_name);
    ctx.bind(fields, "__doc__", NONE);
    let object = ctx.builtin("object");
    let owner = ctx.allocate_class("md5".into(), fields, vec![object])?;
    let Value::Class { hash_type, .. } = ctx.heap.get_mut(owner) else {
        unreachable!()
    };
    *hash_type = Some(CLASS);
    let Value::Scope(frame) = ctx.heap.get_mut(fields) else {
        unreachable!()
    };
    frame.class_namespace.as_mut().unwrap().slot_names = Some([].into());
    for name in [
        "copy",
        "update",
        "digest",
        "hexdigest",
        "name",
        "block_size",
        "digest_size",
    ] {
        let method = ctx.alloc(Value::HashMethod { owner, name });
        let member = if matches!(name, "name" | "block_size" | "digest_size") {
            ctx.alloc(Value::GetSetDescriptor {
                get: method,
                set: None,
                owner: CLASS,
                name,
            })
        } else {
            method
        };
        ctx.bind(fields, name, member);
    }
    let callable = ctx.alloc(Value::HashMethod { owner, name: "md5" });
    let factory = ctx.alloc(Value::Method {
        callable,
        receiver: module,
        explicit: false,
    });
    ctx.bind(scope, "md5", factory);
    ctx.bind(scope, "MD5Type", owner);
    let threshold = ctx.int(2048);
    ctx.bind(scope, "_GIL_MINSIZE", threshold);
    Ok(())
}

/// The wrapper retains its declaring class independently of the public module dictionary.
fn instance(ctx: &mut Context, owner: Id, state: Id) -> Id {
    let hash = ctx.instance(owner, None);
    let Value::Instance { scalar, .. } = ctx.heap.get_mut(hash) else {
        unreachable!()
    };
    *scalar = Some(state);
    hash
}

pub(in crate::dynamic) fn check_receiver(
    ctx: &Context,
    owner: Id,
    name: &str,
    receiver: Id,
) -> Result<()> {
    if ctx.heap.get(receiver).instance_class() == Some(owner) {
        Ok(())
    } else {
        let actual = match ctx
            .heap
            .get(receiver)
            .instance_class()
            .and_then(|class| ctx.heap.get(class).hash_type())
        {
            Some(name) => name,
            None => ctx.type_name(receiver),
        };
        Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '{CLASS}' objects doesn't apply to a '{actual}' object"
            ),
        ))
    }
}

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, remaining)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method md5.{name}() needs an argument"),
        ));
    };
    if name == "md5" {
        let state = super::fixed_hash(ctx, "md5", Some(CLASS), remaining, kwargs)?;
        return Ok(instance(ctx, owner, state));
    }
    check_receiver(ctx, owner, name, receiver)?;
    let state = ctx.scalar_value(receiver);
    if matches!(name, "name" | "block_size" | "digest_size") {
        return ctx.lookup_attr(state, name);
    }
    let mut raw_args = args.to_vec();
    raw_args[0] = state;
    let value = super::provider_method(ctx, CLASS, name, &raw_args, kwargs)?;
    Ok(if name == "copy" {
        instance(ctx, owner, value)
    } else {
        value
    })
}
