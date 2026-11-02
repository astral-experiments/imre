use std::ffi::CString;

use super::{Context, Error, Id, NONE, Result};

pub(super) fn setlocale(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
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
    let module = ctx.module("_locale")?;
    let mut error = Error::new(
        "locale.Error",
        if name.is_none() {
            "locale query failed"
        } else {
            "unsupported locale setting"
        },
    );
    error.class = ctx.lookup_attr(module, "Error")?;
    Err(error)
}
