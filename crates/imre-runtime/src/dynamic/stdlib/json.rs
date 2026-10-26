use std::collections::HashSet;
use std::fmt::Write;

use super::{Context, Error, Id, Result, Value};

use crate::objects::write_json_string;

pub(super) fn dumps(ctx: &Context, id: Id, ascii: bool) -> Result<String> {
    let mut out = String::new();
    write_value(ctx, id, &mut HashSet::new(), &mut out, ascii)?;
    Ok(out)
}

fn write_value(
    ctx: &Context,
    id: Id,
    active: &mut HashSet<Id>,
    out: &mut String,
    ascii: bool,
) -> Result<()> {
    match ctx.heap.get(id) {
        Value::None => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Int(value) => write!(out, "{value}").unwrap(),
        Value::Str(value) => write_json_string(out, value, ascii)?,
        container => {
            if active.len() >= 512 {
                return Err(Error::new(
                    "RecursionError",
                    "maximum JSON nesting exceeded",
                ));
            }
            if !active.insert(id) {
                return Err(Error::new("ValueError", "Circular reference detected"));
            }
            match container {
                Value::List(items)
                | Value::Tuple(items)
                | Value::NamedTuple { values: items, .. } => {
                    out.push('[');
                    for (i, &item) in items.iter().enumerate() {
                        if i != 0 {
                            out.push_str(", ");
                        }
                        write_value(ctx, item, active, out, ascii)?;
                    }
                    out.push(']');
                }
                Value::Dict(entries) | Value::DefaultDict { entries, .. } => {
                    out.push('{');
                    for (i, &(key, value)) in entries.iter().enumerate() {
                        if i != 0 {
                            out.push_str(", ");
                        }
                        match ctx.heap.get(key) {
                            Value::Str(s) => write_json_string(out, s, ascii)?,
                            Value::Int(n) => write_json_string(out, &n.to_string(), ascii)?,
                            Value::None => write_json_string(out, "null", ascii)?,
                            Value::Bool(v) => {
                                write_json_string(out, if *v { "true" } else { "false" }, ascii)?
                            }
                            _ => {
                                return Err(Error::new("TypeError", "unsupported JSON object key"));
                            }
                        }
                        out.push_str(": ");
                        write_value(ctx, value, active, out, ascii)?;
                    }
                    out.push('}');
                }
                _ => return Err(Error::new("TypeError", "object is not JSON serializable")),
            }
            active.remove(&id);
        }
    }
    Ok(())
}
