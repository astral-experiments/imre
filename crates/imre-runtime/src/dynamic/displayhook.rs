//! Display an interactive expression through the interpreter's current Python hook.
use super::{Context, Error, Id, NONE, Result, Value, builtins, stdlib::codecs};

impl Context {
    pub(super) fn display_expression(&mut self, value: Id) -> Result<()> {
        self.heap.keep(value);
        let hook = self
            .sys_binding("displayhook")?
            .ok_or_else(|| Error::new("RuntimeError", "lost sys.displayhook"))?;
        self.heap.keep(hook);
        self.call(hook, vec![value], Vec::new())?;
        Ok(())
    }
}

fn write(ctx: &mut Context, stream: Id, text: Id) -> Result<()> {
    ctx.heap.keep(text);
    ctx.heap.keep(stream);
    let writer = ctx.lookup_attr(stream, "write")?;
    ctx.heap.keep(writer);
    ctx.call(writer, vec![text], Vec::new())?;
    Ok(())
}

fn write_repr(ctx: &mut Context, stream: Id, value: Id) -> Result<()> {
    // PyFile_WriteObject pins the writer before the value's __repr__ can replace it.
    let writer = ctx.lookup_attr(stream, "write")?;
    ctx.heap.keep(writer);
    let text = builtins::call(ctx, "repr", &[value], &[])?;
    let text = if matches!(ctx.heap.get(stream), Value::TextFile(_)) {
        ctx.scalar_value(text)
    } else {
        text
    };
    ctx.heap.keep(text);
    ctx.call(writer, vec![text], Vec::new())?;
    Ok(())
}

fn write_unencodable(ctx: &mut Context, stream: Id, value: Id) -> Result<()> {
    if let Value::TextFile(file) = ctx.heap.get(stream) {
        // The original native stdout writes through its binary buffer before buffered text.
        let file = file.clone();
        if !file.has_encoding() {
            return Err(Error::new(
                "TypeError",
                "bad argument type for built-in operation",
            ));
        }
        let text = builtins::call(ctx, "repr", &[value], &[])?;
        file.write_escaped_display(ctx.str_value(text)?)?;
        return Ok(());
    }
    let encoding = ctx.lookup_attr(stream, "encoding")?;
    ctx.heap.keep(encoding);
    ctx.str_value(encoding)?;
    let repr = builtins::call(ctx, "repr", &[value], &[])?;
    ctx.heap.keep(repr);
    let errors = ctx.text("backslashreplace");
    let bytes = codecs::encode_text(ctx, repr, Some(encoding), Some(errors))?;
    let bytes = ctx.alloc(Value::Bytes(bytes));
    ctx.heap.keep(bytes);
    match ctx.lookup_attr(stream, "buffer") {
        Ok(buffer) => write(ctx, buffer, bytes),
        Err(error) if ctx.is_attribute_error(&error) => {
            let codec = crate::objects::Codec::lookup(ctx.str_value(encoding)?)?;
            let text = codecs::decode_bytes(ctx, codec, bytes, None)?;
            write(ctx, stream, text)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn default_hook(ctx: &mut Context, value: Id) -> Result<Id> {
    ctx.heap.keep(value);
    let key = ctx.text("builtins");
    let builtins = ctx
        .registered_module(key)?
        .ok_or_else(|| Error::new("RuntimeError", "lost builtins module"))?;
    if value == NONE {
        return Ok(NONE);
    }
    ctx.heap.keep(builtins);
    ctx.set_attr(builtins, "_", NONE)?;
    let stream = ctx
        .sys_binding("stdout")?
        .filter(|&stream| stream != NONE)
        .ok_or_else(|| Error::new("RuntimeError", "lost sys.stdout"))?;
    ctx.heap.keep(stream);
    if let Err(error) = write_repr(ctx, stream, value) {
        if !ctx.is_builtin_error(&error, "UnicodeEncodeError") {
            return Err(error);
        }
        write_unencodable(ctx, stream, value)?;
    }
    let newline = ctx.text("\n");
    write(ctx, stream, newline)?;
    ctx.set_attr(builtins, "_", value)?;
    Ok(NONE)
}
