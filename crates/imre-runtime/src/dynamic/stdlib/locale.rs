use std::ffi::CString;

use super::{Context, Error, Id, NONE, Result, Value};

/// Bind setlocale to this module and keep its exception class across namespace edits.
pub(in super::super) fn install(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let fields = ctx.scope("locale", None);
    let module_name = ctx.text("locale");
    ctx.bind(fields, "__module__", module_name);
    let qualname = ctx.text("Error");
    ctx.bind(fields, "__qualname__", qualname);
    ctx.bind(fields, "__doc__", NONE);
    let base = ctx.builtin("Exception");
    let class = ctx.allocate_class("Error".into(), fields, vec![base])?;
    ctx.install_weakref_descriptor(class, fields);
    let Value::Module { module_state, .. } = ctx.heap.get_mut(module) else {
        unreachable!()
    };
    *module_state = Some(class);
    ctx.bind(scope, "Error", class);

    let callable = ctx.builtin("_locale.setlocale");
    let function = ctx.alloc(Value::Method {
        callable,
        receiver: module,
        explicit: false,
    });
    ctx.bind(scope, "setlocale", function);
    Ok(())
}

pub(super) fn setlocale(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&module, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "setlocale requires a module receiver",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_locale.setlocale() takes no keyword arguments",
        ));
    }
    let Some(&category) = args.first() else {
        return Err(Error::new(
            "TypeError",
            "setlocale expected at least 1 argument, got 0",
        ));
    };
    if args.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!("setlocale expected at most 2 arguments, got {}", args.len()),
        ));
    }
    let category = super::os::c_int(ctx, category)?;
    let name = match args.get(1).copied() {
        None | Some(NONE) => None,
        Some(id) => {
            if ctx.string_storage(id).is_none() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "setlocale() argument 2 must be str or None, not {}",
                        ctx.type_name(id)
                    ),
                ));
            }
            let bytes = ctx.encode_utf8_codepoints(id, false, "strict")?;
            Some(
                CString::new(bytes)
                    .map_err(|_| Error::new("ValueError", "embedded null character"))?,
            )
        }
    };
    if let Some(result) = crate::sys::set_locale(category, name.as_deref()) {
        return result
            .map(|name| ctx.text(name))
            .map_err(|error| ctx.decode_error(error));
    }
    let Value::Module {
        module_state: Some(class),
        ..
    } = *ctx.heap.get(module)
    else {
        return Err(Error::new(
            "TypeError",
            "setlocale requires a locale module",
        ));
    };
    let mut error = Error::new(
        "locale.Error",
        if name.is_none() {
            "locale query failed"
        } else {
            "unsupported locale setting"
        },
    );
    error.class = class;
    Err(error)
}
