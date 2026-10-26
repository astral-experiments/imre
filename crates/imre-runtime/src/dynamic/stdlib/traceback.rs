//! Render native exception objects through Python's traceback.print_exception interface.

use std::fmt::Write as _;
use std::io::Write as _;

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
    fn format(&self) -> String {
        let mut out = format!(
            "  File \"{}\", line {}, in {}\n",
            self.file, self.line, self.name
        );
        let Ok(contents) = std::fs::read_to_string(self.file) else {
            return out;
        };
        let Some(raw) = usize::try_from(self.line)
            .ok()
            .and_then(|line| line.checked_sub(1))
            .and_then(|i| contents.lines().nth(i))
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
    let exc = argument(args, 0)?;
    let value = option(args, 1, kwargs, "value")?;
    let tb = option(args, 2, kwargs, "tb")?;
    let (value, mut tb) = match (value, tb) {
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
    if let Some(limit) = option(args, 3, kwargs, "limit")?.filter(|&n| n != NONE) {
        let n = ctx.as_integer(limit)?;
        if n >= 0 {
            stack.truncate(usize::try_from(n).unwrap_or(usize::MAX));
        } else {
            let skip = stack
                .len()
                .saturating_sub(usize::try_from(n.unsigned_abs()).unwrap_or(usize::MAX));
            stack.drain(..skip);
        }
    }
    let mut lines = Vec::new();
    if !stack.is_empty() {
        lines.push("Traceback (most recent call last):\n".to_owned());
        let mut i = 0;
        while i < stack.len() {
            let same = stack[i..]
                .iter()
                .take_while(|f| {
                    f.file == stack[i].file && f.name == stack[i].name && f.line == stack[i].line
                })
                .count();
            lines.extend(stack[i..i + same.min(3)].iter().map(FrameLine::format));
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
    let name = ctx.type_name(value);
    let text = match ctx.heap.get(value) {
        Value::Raised(error)
            if error.kind == "KeyError" && matches!(error.args.as_deref(), Some([_])) =>
        {
            ctx.repr(error.args.as_ref().unwrap()[0])
        }
        _ => ctx.display(value),
    };
    lines.push(if text.is_empty() {
        format!("{name}\n")
    } else {
        format!("{name}: {text}\n")
    });
    let destination = match option(args, 4, kwargs, "file")?.filter(|&f| f != NONE) {
        Some(file) => Some(file),
        None => {
            let sys = ctx.module("sys")?;
            match ctx.lookup_attr(sys, "stderr") {
                Ok(file) => Some(file),
                Err(error) if error.kind == "AttributeError" => None,
                Err(error) => return Err(error),
            }
        }
    };
    if let Some(file) = destination {
        for line in lines {
            let write = ctx.lookup_attr(file, "write")?;
            let line = ctx.text(line);
            ctx.call(write, vec![line], Vec::new())?;
            let write = ctx.lookup_attr(file, "write")?;
            let end = ctx.text("");
            ctx.call(write, vec![end], Vec::new())?;
        }
    } else {
        let mut dst = crate::sys::stderr_writer();
        for line in lines {
            dst.write_all(line.as_bytes()).map_err(super::os_error)?;
        }
    }
    Ok(NONE)
}
