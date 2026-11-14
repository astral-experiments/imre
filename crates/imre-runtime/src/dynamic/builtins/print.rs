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

pub(super) fn write(ctx: &mut Context, file: Id, value: Id) -> Result<()> {
    // PyFile_WriteObject obtains the writer before invoking the value's __str__.
    let before_writer = ctx.heap.temporary_mark();
    let writer = ctx.lookup_attr(file, "write")?;
    ctx.heap.keep(writer);
    let before_string = ctx.heap.temporary_mark();
    let result = text::construct(ctx, "str", &[value], &[])
        .and_then(|string| ctx.call(writer, vec![string], Vec::new()));
    // PyFile_WriteObject drops the rendered string, the writer, then the ignored result.
    let result = ctx.release_with_result(before_string, result);
    ctx.release_with_result(before_writer, result)?;
    ctx.discard_temporaries(before_writer, None);
    Ok(())
}

pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [sep, end, file, flush_arg] = bind("print", &[], kwargs, ["sep", "end", "file", "flush"])?;
    let should_flush = ctx.optional_truth(flush_arg, false)?;
    let file = match file.filter(|&file| file != NONE) {
        Some(file) => file,
        None => match ctx.sys_binding("stdout")? {
            Some(NONE) => return Ok(NONE),
            Some(file) => file,
            None => return Err(Error::new("RuntimeError", "lost sys.stdout")),
        },
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
    if should_flush {
        super::flush_stream(ctx, file)?;
    }
    Ok(NONE)
}
