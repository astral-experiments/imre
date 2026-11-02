//! Exchange one logical line's state with the Python-owned readline iterator.

use super::super::{Context, Error, Id, NONE, Result, Value, builtins};
use super::{Batch, record::Lines};

fn nonnegative(ctx: &Context, value: Id) -> Result<i64> {
    ctx.as_int(value)?
        .to_i64()
        .filter(|n| *n >= 0)
        .ok_or_else(|| Error::new("OverflowError", "tokenizer offset is out of range"))
}

fn position(ctx: &mut Context, (row, column): (i64, i64)) -> Id {
    let row = ctx.int(row);
    let column = ctx.int(column);
    ctx.alloc(Value::Tuple(vec![row, column]))
}

pub(in super::super) fn scan(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let &[source, base, indents, extra, eof, skipped, trailing] = args else {
        return Err(Error::new(
            "TypeError",
            "scan requires seven positional arguments",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new("TypeError", "scan takes no keyword arguments"));
    }

    let base = nonnegative(ctx, base)?;
    let skipped = usize::try_from(nonnegative(ctx, skipped)?)
        .map_err(|_| Error::new("OverflowError", "tokenizer offset is out of range"))?;
    let extra = ctx.truth(extra)?;
    let eof = ctx.truth(eof)?;
    let trailing = ctx.truth(trailing)?;
    let source = ctx.str_value(source)?;
    // Ruff represents byte offsets with u32. Leave room for its implicit final
    // newline, and for the one-based row after the source's last physical line.
    if source.len() >= u32::MAX as usize || base > i64::MAX - source.len() as i64 - 2 {
        return Err(Error::new("OverflowError", "tokenizer input is too large"));
    }
    let source = source.to_owned();
    let Value::List(indents) = ctx.heap.get(indents) else {
        return Err(Error::new(
            "TypeError",
            "expected a list of indentation prefixes",
        ));
    };
    let indents = indents
        .iter()
        .map(|&id| ctx.str_value(id).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut batch = Batch {
        lines: Lines::new(&source, base),
        records: Vec::new(),
        indents,
        extra,
        more: false,
        error: None,
    };
    if trailing {
        batch.trailing(source.len());
    } else {
        batch.read(&source, base, eof);
    }

    let mut previous = None;
    let mut records = Vec::with_capacity(batch.records.len().saturating_sub(skipped));
    for r in batch.records.into_iter().skip(skipped) {
        let ty = ctx.int(r.ty);
        let text = ctx.text(r.text);
        let start = position(ctx, r.start);
        let end = position(ctx, r.end);
        // Tokens on a physical line share the same borrowed slice.
        let line = match previous {
            Some((s, id)) if std::ptr::eq(s, r.line) => id,
            _ => ctx.text(r.line),
        };
        previous = Some((r.line, line));
        records.push(ctx.alloc(Value::Tuple(vec![ty, text, start, end, line])));
    }
    let records = ctx.alloc(Value::List(records));
    let prefixes = batch.indents.into_iter().map(|s| ctx.text(s)).collect();
    let prefixes = ctx.alloc(Value::List(prefixes));
    let failure = match batch.error {
        None => NONE,
        Some(failure) => failure.boxed(ctx)?,
    };
    Ok(ctx.alloc(Value::Tuple(vec![
        records,
        prefixes,
        builtins::boolean(batch.more),
        failure,
    ])))
}
