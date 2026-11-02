use super::{
    Coder, Context, Error, Id, NONE, Result, StreamCodec, Wrapper, flag, invoke, validate_newline,
};

/// Convert a Python str to the UTF-8 C string used by the codec and newline APIs.
/// The public encoding/errors attributes retain the original Python object.
fn option_text(ctx: &mut Context, value: Id) -> Result<String> {
    if let Ok(s) = ctx.str_value(value) {
        return Ok(s.split('\0').next().unwrap_or_default().to_owned());
    }
    let bytes = ctx.encode_utf8_codepoints(value, false, "strict")?;
    let s = std::str::from_utf8(&bytes).expect("validated UTF-8");
    Ok(s.split('\0').next().unwrap_or_default().to_owned())
}

fn setting_flag(
    ctx: &mut Context,
    receiver: Id,
    option: Id,
    get: impl FnOnce(&Wrapper) -> bool,
) -> Result<bool> {
    if option != NONE {
        return Ok(ctx.index_c_long(option)? != 0);
    }
    Ok(Wrapper::from_receiver(ctx, receiver).is_ok_and(|w| get(&w)))
}

fn release_option(ctx: &mut Context, old: Id, new: Id) -> Result<()> {
    if old != new {
        ctx.heap.weak_edge_removed(old);
        super::super::release_child(ctx, Some(old))?;
    }
    Ok(())
}

pub(super) fn call(
    ctx: &mut Context,
    receiver: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            "reconfigure() takes no positional arguments",
        ));
    }
    if kwargs.len() > 5 {
        return Err(Error::new(
            "TypeError",
            format!(
                "reconfigure() takes at most 5 keyword arguments ({} given)",
                kwargs.len()
            ),
        ));
    }
    let (mut encoding, mut errors, mut newline, mut line_buffering, mut write_through) =
        (NONE, NONE, None, NONE, NONE);
    for (key, value) in kwargs {
        match key.as_str() {
            "encoding" => encoding = *value,
            "errors" => errors = *value,
            "newline" => newline = Some(*value),
            "line_buffering" => line_buffering = *value,
            "write_through" => write_through = *value,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("reconfigure() got an unexpected keyword argument '{key}'"),
                ));
            }
        }
    }
    for (name, value) in [
        ("encoding", encoding),
        ("errors", errors),
        ("newline", newline.unwrap_or(NONE)),
    ] {
        if value != NONE && ctx.string_storage(value).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "reconfigure() argument '{name}' must be str or None, not {}",
                    ctx.type_name(value)
                ),
            ));
        }
    }
    let change = encoding != NONE || errors != NONE || newline.is_some();
    if change
        && Wrapper::from_receiver(ctx, receiver)
            .is_ok_and(|w| w.input.borrow().decoded_chunk_nonempty.is_some())
    {
        return Err(Error::new(
            "UnsupportedOperation",
            "It is not possible to set the encoding or newline of stream after the first read",
        ));
    }
    let newline = newline
        .map(|value| -> Result<Option<&'static str>> {
            if value == NONE {
                return Ok(None);
            }
            Ok(Some(validate_newline(&option_text(ctx, value)?)?))
        })
        .transpose()?;
    let line_buffering = setting_flag(ctx, receiver, line_buffering, |w| w.line_buffering.get())?;
    let write_through = setting_flag(ctx, receiver, write_through, |w| w.write_through.get())?;
    invoke(ctx, receiver, "flush", vec![])?;
    let w = Wrapper::from_receiver(ctx, receiver)?;
    if let Some(nl) = newline {
        w.newline.set(nl);
    }
    if change {
        if encoding == NONE {
            encoding = w.encoding.get();
            if errors == NONE {
                errors = w.errors.get();
            }
        } else {
            if ctx.str_value(encoding).is_ok_and(|s| s == "locale") {
                encoding = ctx.text(crate::sys::file_encoding(Some("locale")).into_owned());
            }
            if errors == NONE {
                errors = ctx.text("strict");
            }
        }
        let name = option_text(ctx, encoding)?;
        let error_mode = option_text(ctx, errors)?;
        let codec = StreamCodec::lookup(&name)?;
        let coder = Coder {
            codec,
            errors: ctx.text(error_mode),
        };
        for id in [encoding, errors, coder.errors] {
            ctx.heap.keep(id);
        }
        if flag(ctx, w.binary()?, "readable")? {
            let w = Wrapper::from_receiver(ctx, receiver)?;
            w.decoder.set(coder);
            w.decoder_newline.set(w.newline.get());
            w.readable.set(true);
            w.input.borrow_mut().reset_decoder();
        }
        let w = Wrapper::from_receiver(ctx, receiver)?;
        if flag(ctx, w.binary()?, "writable")? {
            let w = Wrapper::from_receiver(ctx, receiver)?;
            w.encoder.set(coder);
            w.writable.set(true);
            w.utf8_signature_pending.set(true);
        }
        let w = Wrapper::from_receiver(ctx, receiver)?;
        release_option(ctx, w.encoding.replace(encoding), encoding)?;
        // Either removed str subclass can reenter reconfigure or reinitialize the wrapper.
        let w = Wrapper::from_receiver(ctx, receiver)?;
        release_option(ctx, w.errors.replace(errors), errors)?;
        let w = Wrapper::from_receiver(ctx, receiver)?;
        if w.writable.get() && w.seekable {
            w.write_start.set(true);
            let position = Wrapper::invoke_buffer(ctx, receiver, "tell", vec![])?;
            let zero = ctx.int(0);
            let beginning = ctx.checked_equal(position, zero)?;
            if !beginning {
                let w = Wrapper::from_receiver(ctx, receiver)?;
                w.write_start.set(false);
                w.utf8_signature_pending.set(false);
            }
        }
    }
    let w = Wrapper::from_receiver(ctx, receiver)?;
    w.line_buffering.set(line_buffering);
    w.write_through.set(write_through);
    Ok(NONE)
}
