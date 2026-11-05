//! Adapt a boxed Python iterator to the row parser also used by typed readers.
use super::{Context, Error, Id, NONE, Result, Value};
use crate::dynamic::operators::surrogate_repr;
use crate::objects::csv::{Dialect, Field, Row};
use crate::objects::exceptions::{Kind, string_repr};

fn character(ctx: &Context, id: Id) -> Result<u32> {
    if id == NONE {
        return Ok(u32::MAX);
    }
    let mut chars = ctx.str_value(id)?.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(ch as u32),
        _ => Err(Error::new(
            "TypeError",
            "expected one CSV dialect character",
        )),
    }
}

fn integer(ctx: &Context, id: Id) -> Result<i64> {
    ctx.as_int(id)?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

/// Pull only the input elements needed for this record, including separate quoted lines.
pub(super) fn read_row(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let &[
        next_line,
        generation,
        delimiter,
        quote,
        escape,
        flags,
        mode,
        error_class,
    ] = args
    else {
        return Err(Error::new("TypeError", "read_row requires eight arguments"));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "read_row takes no keyword arguments",
        ));
    }
    if delimiter == NONE {
        return Err(Error::new("TypeError", "expected one CSV delimiter"));
    }
    let dialect = Dialect::new(
        character(ctx, delimiter)?,
        character(ctx, quote)?,
        character(ctx, escape)?,
        integer(ctx, flags)? as u32,
    );
    let mode = integer(ctx, mode)?;
    let mut fields = Vec::new();
    let mut row = Row::new(|field, quoted| {
        let numeric = !quoted && matches!(mode, 2 | 4);
        let value = match field {
            Field::Text(text) if !quoted && text.is_empty() && matches!(mode, 4 | 5) => Value::None,
            Field::Text(text) if numeric && !text.is_empty() => {
                let value = crate::floats::parse(&text).ok_or_else(|| {
                    crate::Error::new(
                        Kind::ValueError,
                        format!(
                            "could not convert string to float: {}",
                            string_repr(&text).as_str()
                        ),
                    )
                })?;
                Value::Float(value)
            }
            Field::Surrogates(points) if numeric => {
                return Err(crate::Error::new(
                    Kind::ValueError,
                    format!(
                        "could not convert string to float: {}",
                        surrogate_repr(&points)
                    ),
                ));
            }
            Field::Text(text) => Value::Str(text.into()),
            Field::Surrogates(points) => Value::SurrogateStr(points),
        };
        fields
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        fields.push(value);
        Ok(())
    });
    let parse_error = |err| {
        let mut err = Error::from(err);
        if err.kind == "_csv.Error" {
            err.class = error_class;
        }
        err
    };
    loop {
        match ctx.call(next_line, vec![generation], Vec::new()) {
            Ok(line) => {
                let Value::Tuple(items) = ctx.heap.get(line) else {
                    return Err(Error::new("TypeError", "expected (line, limit)"));
                };
                let &[line, limit] = items.as_slice() else {
                    return Err(Error::new("TypeError", "expected (line, limit)"));
                };
                let limit = integer(ctx, limit)?;
                let complete = match ctx.string_storage(line).map(|id| ctx.heap.get(id)) {
                    Some(Value::Str(text)) => {
                        row.line(&dialect, text.chars().map(u32::from), limit)
                    }
                    Some(Value::SurrogateStr(points)) => {
                        row.line(&dialect, points.iter().copied(), limit)
                    }
                    _ => return Err(Error::new("TypeError", "expected str")),
                };
                if complete.map_err(parse_error)? {
                    break;
                }
            }
            Err(err) if ctx.is_stop_iteration(&err) => {
                if !row.eof(&dialect).map_err(parse_error)? {
                    return Err(Error::new("StopIteration", ""));
                }
                break;
            }
            Err(err) => return Err(err),
        }
    }
    let ids = fields
        .into_iter()
        .map(|value| match value {
            Value::None => NONE,
            other => ctx.alloc(other),
        })
        .collect();
    Ok(ctx.alloc(Value::List(ids)))
}
