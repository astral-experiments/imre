//! Format native exception objects and write their tracebacks to Python streams.

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

pub(super) fn print_active(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    call_with_active_exception(
        ctx,
        args,
        kwargs,
        "print_exc",
        "print_exception",
        [
            ("limit", NONE),
            ("file", NONE),
            ("chain", super::super::TRUE),
        ],
    )?;
    Ok(NONE)
}

pub(super) fn format_active(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let parts = call_with_active_exception(
        ctx,
        args,
        kwargs,
        "format_exc",
        "format_exception",
        [("limit", NONE), ("chain", super::super::TRUE)],
    )?;
    ctx.heap.keep(parts);
    let empty = ctx.text("");
    let join = ctx.lookup_attr(empty, "join")?;
    ctx.call(join, vec![parts], Vec::new())
}

fn call_with_active_exception<const N: usize>(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    name: &str,
    target: &str,
    mut parameters: [(&str, Id); N],
) -> Result<Id> {
    for ((_, value), arg) in parameters.iter_mut().zip(args) {
        *value = *arg;
    }
    for (key, argument) in kwargs {
        let Some(index) = parameters.iter().position(|(param, _)| param == key) else {
            return Err(Error::new(
                "TypeError",
                format!("{name}() got an unexpected keyword argument '{key}'"),
            ));
        };
        if index < args.len() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() got multiple values for argument '{key}'"),
            ));
        }
        parameters[index].1 = *argument;
    }
    if args.len() > N {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes from 0 to {N} positional arguments but {} were given",
                args.len()
            ),
        ));
    }
    let traceback = ctx.module("traceback")?;
    let function = ctx.lookup_attr(traceback, target)?;
    // Looking up the current exception can replace the callable and trigger collection.
    ctx.heap.keep(function);
    let sys = ctx.module("sys")?;
    let current = ctx.lookup_attr(sys, "exception")?;
    let exception = ctx.call(current, Vec::new(), Vec::new())?;
    let options = parameters
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    ctx.call(function, vec![exception], options)
}

fn render(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    chain_index: usize,
) -> Result<Vec<String>> {
    if let Some(colorize) = kwargs
        .iter()
        .find(|(key, _)| key == "colorize")
        .map(|(_, value)| *value)
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
    let chain = option(args, chain_index, kwargs, "chain")?
        .map(|v| ctx.truth(v))
        .transpose()?
        .unwrap_or(true);
    let mut sections = Vec::new();
    let mut seen = std::collections::HashSet::from([value]);
    let (mut current, mut current_tb) = (value, tb);
    let mut separator: Option<&str> = None;
    loop {
        // Exception text is captured from the outermost error inwards. Its hook can
        // change the cause, drop references, and run Python's garbage collector.
        ctx.heap.keep(current);
        let mut lines = Vec::new();
        format_exception(ctx, current, current_tb, limit, &mut lines)?;
        if let Some(separator) = separator {
            lines.push(separator.to_owned());
        }
        sections.push(lines);
        let Value::Raised(error) = ctx.heap.get(current) else {
            break;
        };
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
        current = prior;
        current_tb = ctx.exception_traceback(prior)?;
        separator = Some(message);
    }
    if !chain {
        sections.truncate(1);
    }
    Ok(sections.into_iter().rev().flatten().collect())
}

pub(super) fn format(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    for (name, _) in kwargs {
        if ["value", "tb", "limit", "chain"]
            .iter()
            .take(args.len().saturating_sub(1))
            .any(|param| param == name)
        {
            return Err(Error::new(
                "TypeError",
                format!("format_exception() got multiple values for argument '{name}'"),
            ));
        }
    }
    if args.is_empty() {
        return Err(Error::new(
            "TypeError",
            "format_exception() missing 1 required positional argument: 'exc'",
        ));
    }
    if args.len() > 5 {
        return Err(Error::new(
            "TypeError",
            format!(
                "format_exception() takes from 1 to 5 positional arguments but {} were given",
                args.len()
            ),
        ));
    }
    let lines = render(ctx, args, kwargs, 4)?;
    let values = lines.into_iter().map(|line| ctx.text(line)).collect();
    Ok(ctx.alloc(Value::List(values)))
}

pub(super) fn print(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let lines = render(ctx, args, kwargs, 5)?;
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
    ctx: &mut Context,
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
    let convert = ctx.builtin("str");
    let text = match ctx.call(convert, vec![value], Vec::new()) {
        Ok(result) => ctx.str_value(result)?.to_owned(),
        Err(_) => "<exception str() failed>".to_owned(),
    };
    let (module, dot, name) = exception_label(ctx, value)?;
    let separator = if text.is_empty() { "" } else { ": " };
    lines.push(format!("{module}{dot}{name}{separator}{text}\n"));
    Ok(())
}

/// Capture the exception class name after its string hook, as traceback does.
fn exception_label(ctx: &mut Context, value: Id) -> Result<(&str, &str, &str)> {
    let class = match ctx.heap.get(value) {
        Value::Raised(error) => error.class,
        _ => None,
    };
    let Some(class) = class else {
        return Ok(("", "", ctx.type_name(value)));
    };
    let Some(name) = ctx.own_class_attr(class, "__qualname__") else {
        return Ok(("", "", ctx.type_name(value)));
    };
    let Some(module) = ctx.own_class_attr(class, "__module__") else {
        return Ok(("", "", ctx.str_value(name)?));
    };
    if matches!(ctx.heap.get(module), Value::Str(_)) && matches!(ctx.heap.get(name), Value::Str(_))
    {
        let (prefix, dot) = match ctx.str_value(module)? {
            "__main__" | "builtins" => ("", ""),
            other => (other, "."),
        };
        return Ok((prefix, dot, ctx.str_value(name)?));
    }

    // A module-name hook can replace both attributes and run a collection.
    ctx.heap.reserve_temporaries(2)?;
    ctx.heap.keep(name);
    ctx.heap.keep(module);
    for literal in ["__main__", "builtins"] {
        let reserved = label_literal(ctx, literal)?;
        if ctx.checked_equal(reserved, module)? {
            return Ok(("", "", displayed_label(ctx, name)?));
        }
    }
    let module = if ctx.string_storage(module).is_some() {
        module
    } else {
        label_literal(ctx, "<unknown>")?
    };
    if !label_slot(ctx, module, "__add__") && !label_slot(ctx, name, "__radd__") {
        return Ok((ctx.str_value(module)?, ".", ctx.str_value(name)?));
    }
    let dot = label_literal(ctx, ".")?;
    let prefix = label_add(ctx, module, dot)?;
    ctx.heap.reserve_temporaries(1)?;
    ctx.heap.keep(prefix);
    let full = label_add(ctx, prefix, name)?;
    Ok(("", "", displayed_label(ctx, full)?))
}

/// Retain constants separately from the class's Python-owned labels.
fn label_literal(ctx: &mut Context, value: &str) -> Result<Id> {
    ctx.heap
        .source_string_constant(value)
        .ok_or_else(|| Error::new("MemoryError", ""))
}

/// Built-in string concatenation can use its immutable storage directly.
fn label_slot(ctx: &Context, value: Id, slot: &str) -> bool {
    ctx.heap
        .get(value)
        .instance_class()
        .and_then(|class| ctx.class_attr(class, slot))
        .is_some()
}

/// Add label parts once through Python slots before using inherited string addition.
fn label_add(ctx: &mut Context, left: Id, right: Id) -> Result<Id> {
    if let Some(value) = ctx.binary_special("+", left, right)? {
        return Ok(value);
    }
    if !label_slot(ctx, left, "__add__")
        && let Some((a, b)) = ctx.string_storage(left).zip(ctx.string_storage(right))
    {
        return ctx.binary("+", a, b);
    }
    Err(Error::new(
        "TypeError",
        format!(
            "unsupported operand type(s) for +: '{}' and '{}'",
            ctx.type_name(left),
            ctx.type_name(right)
        ),
    ))
}

/// The final traceback line applies Python's default format specification.
fn displayed_label(ctx: &mut Context, label: Id) -> Result<&str> {
    if matches!(ctx.heap.get(label), Value::Str(_)) {
        return ctx.str_value(label);
    }
    let displayed = if let Some(method) = ctx.bound_slot(label, "__format__")? {
        let spec = label_literal(ctx, "")?;
        ctx.call(method, super::super::copy_ids(&[spec])?, Vec::new())?
    } else {
        super::super::builtins::call(ctx, "format", &[label], &[])?
    };
    if ctx.string_storage(displayed).is_none() {
        return Err(Error::new(
            "TypeError",
            format!(
                "__format__ must return a str, not {}",
                ctx.type_name(displayed)
            ),
        ));
    }
    ctx.str_value(displayed)
}
