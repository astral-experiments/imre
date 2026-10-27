//! Write Python values to the stream selected for this print call.

use super::{Context, Error, Id, NONE, Result, bind, text, type_error};

fn separator(ctx: &mut Context, name: &str, value: Option<Id>, default: &str) -> Result<Id> {
    match value.filter(|&value| value != NONE) {
        None => Ok(ctx.text(default)),
        Some(value) if ctx.string_storage(value).is_some() => Ok(value),
        Some(value) => Err(type_error(format!(
            "{name} must be None or a string, not {}",
            ctx.type_name(value)
        ))),
    }
}

fn write(ctx: &mut Context, file: Id, value: Id) -> Result<()> {
    // PyFile_WriteObject obtains the writer before invoking the value's __str__.
    let writer = ctx.lookup_attr(file, "write")?;
    ctx.heap.keep(writer);
    let string = text::construct(ctx, "str", &[value], &[])?;
    ctx.call(writer, vec![string], Vec::new())?;
    Ok(())
}

pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [sep, end, file, flush] = bind("print", &[], kwargs, ["sep", "end", "file", "flush"])?;
    let flush = ctx.optional_truth(flush, false)?;
    let file = match file.filter(|&file| file != NONE) {
        Some(file) => file,
        None => {
            let sys = ctx.module("sys")?;
            match ctx.lookup_attr(sys, "stdout") {
                Ok(NONE) => return Ok(NONE),
                Ok(file) => file,
                Err(error) if ctx.is_attribute_error(&error) => {
                    return Err(Error::new("RuntimeError", "lost sys.stdout"));
                }
                Err(error) => return Err(error),
            }
        }
    };
    // A user conversion can replace sys.stdout and trigger collection during the call.
    ctx.heap.keep(file);
    let sep = separator(ctx, "sep", sep, " ")?;
    let end = separator(ctx, "end", end, "\n")?;
    for (i, &value) in args.iter().enumerate() {
        if i != 0 {
            write(ctx, file, sep)?;
        }
        write(ctx, file, value)?;
    }
    write(ctx, file, end)?;
    if flush {
        let method = ctx.lookup_attr(file, "flush")?;
        ctx.call(method, Vec::new(), Vec::new())?;
    }
    Ok(NONE)
}
