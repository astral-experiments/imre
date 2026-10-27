//! Render native exception objects through Python's traceback.print_exception interface.

use std::collections::HashMap;
use std::fmt::Write as _;

use icu_properties::{CodePointMapData, props::EastAsianWidth};

use super::{Context, Error, Id, NONE, Result, Value, argument, option};

struct FrameLine<'a> {
    file: &'a str,
    name: &'a str,
    line: i64,
    span: Option<[u32; 3]>,
}

fn display_width(text: &str) -> usize {
    if text.is_ascii() {
        return text.len();
    }
    let widths = CodePointMapData::<EastAsianWidth>::new();
    text.chars()
        .map(|ch| match widths.get(ch) {
            EastAsianWidth::Wide | EastAsianWidth::Fullwidth => 2,
            _ => 1,
        })
        .sum()
}

impl FrameLine<'_> {
    fn format(&self, source: Option<&str>) -> String {
        let mut out = format!(
            "  File \"{}\", line {}, in {}\n",
            self.file, self.line, self.name
        );
        let Some(raw) = usize::try_from(self.line)
            .ok()
            .and_then(|line| line.checked_sub(1))
            .and_then(|i| source?.lines().nth(i))
        else {
            return out;
        };
        let line = raw.trim();
        if line.is_empty() {
            return out;
        }
        let _ = writeln!(out, "    {line}");
        if let Some([start, split, end]) = self.span.map(|s| s.map(|v| v as usize)) {
            let indent = raw.len() - raw.trim_start().len();
            if let (Some(prefix), Some(primary), Some(secondary)) = (
                raw.get(indent..start),
                raw.get(start..split),
                raw.get(split..end),
            ) {
                let _ = writeln!(
                    out,
                    "    {}{}{}",
                    " ".repeat(display_width(prefix)),
                    "~".repeat(display_width(primary)),
                    "^".repeat(display_width(secondary))
                );
            }
        }
        out
    }
}

pub(super) fn print(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if let Some(colorize) = option(args, 6, kwargs, "colorize")?
        && ctx.truth(colorize)?
    {
        return Err(Error::new(
            "NotImplementedError",
            "colored tracebacks are not supported",
        ));
    }
    let exc = argument(args, 0)?;
    let value = option(args, 1, kwargs, "value")?;
    let tb = option(args, 2, kwargs, "tb")?;
    let (value, tb) = match (value, tb) {
        (None, None) if exc == NONE => (NONE, NONE),
        (None, None) if matches!(ctx.heap.get(exc), Value::Raised(_)) => {
            (exc, ctx.exception_traceback(exc)?)
        }
        (None, None) => {
            return Err(Error::new(
                "TypeError",
                format!("Exception expected for value, {} found", ctx.type_name(exc)),
            ));
        }
        (Some(value), Some(tb)) => (value, tb),
        _ => {
            return Err(Error::new(
                "ValueError",
                "Both or neither of value and tb must be given",
            ));
        }
    };
    let limit = match option(args, 3, kwargs, "limit")?.filter(|&l| l != NONE) {
        Some(l) => Some(ctx.as_integer(l)?),
        None => {
            let sys = ctx.module("sys")?;
            match ctx.lookup_attr(sys, "tracebacklimit") {
                Ok(l) if l != NONE => Some(ctx.as_integer(l)?.max(0)),
                Ok(_) => None,
                Err(error) if error.kind == "AttributeError" => None,
                Err(error) => return Err(error),
            }
        }
    };
    let chain = option(args, 5, kwargs, "chain")?
        .map(|v| ctx.truth(v))
        .transpose()?
        .unwrap_or(true);
    let mut causes = vec![(value, tb, None)];
    if chain {
        let mut seen = std::collections::HashSet::from([value]);
        let mut child = value;
        while let Value::Raised(error) = ctx.heap.get(child) {
            let Some(trace) = &error.trace else { break };
            let (prior, message) = if let Some(cause) = trace.cause {
                (
                    cause,
                    "\nThe above exception was the direct cause of the following exception:\n\n",
                )
            } else if let Some(context) = trace.context {
                (
                    context,
                    "\nDuring handling of the above exception, another exception occurred:\n\n",
                )
            } else {
                break;
            };
            if prior == NONE || !seen.insert(prior) {
                break;
            }
            causes.push((prior, ctx.exception_traceback(prior)?, Some(message)));
            child = prior;
        }
    }
    let mut lines = Vec::new();
    for (value, tb, separator) in causes.into_iter().rev() {
        format_exception(ctx, value, tb, limit, &mut lines)?;
        if let Some(separator) = separator {
            lines.push(separator.to_owned());
        }
    }
    let file = match option(args, 4, kwargs, "file")?.filter(|&f| f != NONE) {
        Some(file) => file,
        None => {
            let sys = ctx.module("sys")?;
            ctx.lookup_attr(sys, "stderr")?
        }
    };
    // A write callback can replace sys.stderr and trigger collection before the next line.
    ctx.heap.keep(file);
    for line in lines {
        let write = ctx.lookup_attr(file, "write")?;
        let line = ctx.text(line);
        ctx.call(write, vec![line], Vec::new())?;
        let write = ctx.lookup_attr(file, "write")?;
        let end = ctx.text("");
        ctx.call(write, vec![end], Vec::new())?;
    }
    Ok(NONE)
}

fn format_exception(
    ctx: &Context,
    value: Id,
    mut tb: Id,
    limit: Option<i64>,
    lines: &mut Vec<String>,
) -> Result<()> {
    let mut stack = Vec::new();
    while tb != NONE {
        let Value::Traceback {
            frame,
            next,
            lineno,
            span,
        } = *ctx.heap.get(tb)
        else {
            return Err(Error::new("TypeError", "expected a traceback"));
        };
        let Value::PythonFrame { code, .. } = *ctx.heap.get(frame) else {
            unreachable!()
        };
        let code = ctx.code_object(code);
        stack.push(FrameLine {
            file: ctx.str_value(code.filename)?,
            name: ctx.str_value(code.name)?,
            line: lineno,
            span,
        });
        tb = next;
    }
    if let Some(n) = limit {
        if n >= 0 {
            stack.truncate(usize::try_from(n).unwrap_or(usize::MAX));
        } else {
            let skip = stack
                .len()
                .saturating_sub(usize::try_from(n.unsigned_abs()).unwrap_or(usize::MAX));
            stack.drain(..skip);
        }
    }
    if !stack.is_empty() {
        lines.push("Traceback (most recent call last):\n".to_owned());
        let mut sources = HashMap::new();
        let mut i = 0;
        while i < stack.len() {
            let same = stack[i..]
                .iter()
                .take_while(|f| {
                    f.file == stack[i].file && f.name == stack[i].name && f.line == stack[i].line
                })
                .count();
            for frame in &stack[i..i + same.min(3)] {
                let contents = sources
                    .entry(frame.file)
                    .or_insert_with(|| std::fs::read_to_string(frame.file).ok());
                lines.push(frame.format(contents.as_deref()));
            }
            if same > 3 {
                let count = same - 3;
                lines.push(format!(
                    "  [Previous line repeated {count} more time{}]\n",
                    if count == 1 { "" } else { "s" }
                ));
            }
            i += same;
        }
    }
    let qualname = match ctx.heap.get(value) {
        Value::Raised(error) => error
            .class
            .and_then(|class| ctx.own_class_attr(class, "__qualname__")),
        _ => None,
    };
    let name = qualname
        .and_then(|name| ctx.str_value(name).ok())
        .unwrap_or_else(|| ctx.type_name(value));
    let text = match ctx.heap.get(value) {
        Value::Raised(error) if error.kind == "KeyError" => match error.args.as_deref() {
            Some([key]) => ctx.repr(*key),
            _ => ctx.display(value),
        },
        _ => ctx.display(value),
    };
    lines.push(if text.is_empty() {
        format!("{name}\n")
    } else {
        format!("{name}: {text}\n")
    });
    Ok(())
}
