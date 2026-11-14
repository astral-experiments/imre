//! Native descriptor, frame and signature layout helpers for inspect.
use super::{Context, FALSE, Id, Result, TRUE, Value};

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "_descriptor_get" {
        if let [descriptor, obj] = *args
            && kwargs.is_empty()
        {
            // As in inspect._descriptor_get, class objects supply their own signature.
            if ctx.is_type_object(descriptor) {
                return Ok(descriptor);
            }
            let owner = ctx.exact_type(obj)?;
            return ctx.bind_attribute(descriptor, obj, owner);
        }
        return Err(super::Error::new(
            "TypeError",
            "_descriptor_get expects two positional arguments",
        ));
    }
    let [obj] = super::required_arguments(name, ["obj"], args, kwargs)?;
    match name {
        "_abstract_flags" => {
            let target = ctx.weak_proxy_target(obj)?;
            let abstract_class = match *ctx.heap.get(target) {
                Value::Class {
                    scope, metaclass, ..
                } => {
                    // The default type descriptor takes precedence over a class-body __flags__.
                    if metaclass.is_some_and(|meta| {
                        ctx.class_attr(meta, "__flags__").is_some()
                            || ctx.class_attr(meta, "__getattribute__").is_some()
                    }) {
                        return ctx.lookup_attr(obj, "__flags__");
                    }
                    ctx.scope_data(scope).is_abstract
                }
                _ if ctx.is_type_object(target) => false,
                // isinstance(obj, type) can also succeed through a reported __class__.
                _ => return ctx.lookup_attr(obj, "__flags__"),
            };
            Ok(ctx.int(if abstract_class { 1 << 20 } else { 0 }))
        }
        "_native_data_descriptor" => Ok(if ctx.native_data_descriptor(obj) {
            TRUE
        } else {
            FALSE
        }),
        "_frame_position" => match *ctx.heap.get(obj) {
            Value::Traceback { .. } => Ok(super::traceback::tb_position(ctx, obj)),
            Value::PythonFrame { lineno, .. } => {
                let fields = match ctx.frame_position(obj) {
                    Some(range) => range.map(|v| ctx.int(i64::from(v))),
                    None => [
                        ctx.int(i64::from(lineno)),
                        super::NONE,
                        super::NONE,
                        super::NONE,
                    ],
                };
                Ok(ctx.alloc(Value::Tuple(fields.to_vec())))
            }
            _ => Err(super::Error::new(
                "TypeError",
                "expected a frame or traceback",
            )),
        },
        _ => Err(super::Error::new(
            "SystemError",
            format!("unknown inspect helper {name}"),
        )),
    }
}
