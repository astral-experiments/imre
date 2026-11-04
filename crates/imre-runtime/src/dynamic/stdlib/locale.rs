use std::ffi::CString;

use crate::dynamic::value::NativeModuleState;

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
    *module_state = Some(NativeModuleState::LocaleError(class));
    ctx.bind(scope, "Error", class);

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        for item in crate::sys::langinfo::items() {
            let value = ctx.int(item.value.into());
            ctx.bind(scope, item.name(), value);
        }
        let callable = ctx.builtin("_locale.nl_langinfo");
        let function = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.bind(scope, "nl_langinfo", function);
    }

    let char_max = ctx.int(libc::c_char::MAX.into());
    ctx.bind(scope, "CHAR_MAX", char_max);
    for (name, symbol) in [
        ("setlocale", "_locale.setlocale"),
        ("localeconv", "_locale.localeconv"),
    ] {
        let callable = ctx.builtin(symbol);
        let function = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.bind(scope, name, function);
    }
    Ok(())
}

pub(super) fn localeconv(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_locale.localeconv() takes no keyword arguments",
        ));
    }
    if args.len() != 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "_locale.localeconv() takes no arguments ({} given)",
                args.len().saturating_sub(1)
            ),
        ));
    }
    let fields = crate::sys::locale_conventions().map_err(|err| ctx.decode_error(err))?;
    let entries = fields
        .into_iter()
        .map(|(name, field)| {
            let value = match field {
                crate::sys::Convention::Text(value) => ctx.text(value),
                crate::sys::Convention::Number(value) => ctx.int(value),
                crate::sys::Convention::Grouping(widths) => {
                    let values = widths.into_iter().map(|n| ctx.int(n.into())).collect();
                    ctx.alloc(Value::List(values))
                }
            };
            (ctx.heap.code_name(name), value)
        })
        .collect();
    Ok(ctx.dict(entries))
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
        module_state: Some(NativeModuleState::LocaleError(class)),
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

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(super) fn nl_langinfo(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_locale.nl_langinfo() takes no keyword arguments",
        ));
    }
    let [_, key] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "_locale.nl_langinfo() takes exactly one argument ({} given)",
                args.len().saturating_sub(1)
            ),
        ));
    };
    let key = super::os::c_int(ctx, *key)?;
    let item = crate::sys::langinfo::items()
        .iter()
        .find(|item| item.value == key)
        .ok_or_else(|| Error::new("ValueError", "unsupported langinfo constant"))?;
    let text = item.read().map_err(|err| ctx.decode_error(err))?;
    Ok(ctx.text(text))
}
