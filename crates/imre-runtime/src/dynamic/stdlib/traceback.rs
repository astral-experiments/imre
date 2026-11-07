//! Format native exception objects and write their tracebacks to Python streams.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use icu_properties::{CodePointMapData, props::EastAsianWidth};

use super::{Context, Error, Id, NONE, Result, Value, argument, option};

/// Traceback text is converted to a Python object only at the public return or write boundary.
#[derive(PartialEq, Eq, Hash)]
pub(in crate::dynamic) enum TraceText<'a> {
    Scalar(Cow<'a, str>),
    Points(Cow<'a, [u32]>),
}

impl<'a> TraceText<'a> {
    fn read(ctx: &'a Context, value: Id) -> Result<Self> {
        match ctx.heap.get(ctx.scalar_value(value)) {
            Value::SurrogateStr(points) => Ok(Self::Points(Cow::Borrowed(points))),
            _ => Ok(Self::Scalar(Cow::Borrowed(ctx.str_value(value)?))),
        }
    }

    fn owned(self) -> TraceText<'static> {
        match self {
            Self::Scalar(text) => TraceText::Scalar(Cow::Owned(text.into_owned())),
            Self::Points(points) => TraceText::Points(Cow::Owned(points.into_owned())),
        }
    }

    fn surrounded(&self, prefix: &str, suffix: &str) -> TraceText<'static> {
        match self {
            Self::Scalar(text) => TraceText::Scalar(format!("{prefix}{text}{suffix}").into()),
            Self::Points(points) => {
                let line = prefix
                    .chars()
                    .map(u32::from)
                    .chain(points.iter().copied())
                    .chain(suffix.chars().map(u32::from))
                    .collect::<Vec<_>>();
                TraceText::Points(line.into())
            }
        }
    }

    pub(in crate::dynamic) fn into_python(self, ctx: &mut Context) -> Id {
        match self {
            Self::Scalar(text) => ctx.text(text.into_owned()),
            Self::Points(points) => ctx.text_codepoints(points.into_owned()),
        }
    }
}

struct FrameLine<'a> {
    locals: &'a str,
    file: TraceText<'a>,
    path: Option<&'a Path>,
    embedded_source: Option<&'a str>,
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

impl<'a> FrameLine<'a> {
    fn new(
        ctx: &'a Context,
        code: Id,
        line: i64,
        span: Option<[u32; 3]>,
        locals: &'a str,
        path: Option<&'a Path>,
    ) -> Result<Self> {
        let code = ctx.code_object(code);
        let file = TraceText::read(ctx, code.filename)?;
        // Only native code has the original embedded source at this position.
        let embedded_source = if !matches!(code.body, crate::dynamic::value::Body::Native(_)) {
            None
        } else if matches!(&file, TraceText::Scalar(text) if text == "<string>") {
            ctx.config["command_source"].as_str()
        } else if ctx.config["bundled_root"]
            .as_str()
            .is_some_and(|root| path.is_some_and(|p| p.starts_with(root)))
        {
            ctx.sources
                .values()
                .flatten()
                .find(|module| Some(module.path.as_path()) == path)
                .and_then(|module| module.bundled_text)
                .map(|text| std::str::from_utf8(text).expect("compiler-emitted Python is UTF-8"))
        } else {
            None
        };
        Ok(Self {
            locals,
            file,
            path,
            embedded_source,
            name: ctx.str_value(code.name)?,
            line,
            span,
        })
    }

    fn formatted_suffix(&self, source: Option<&str>) -> String {
        let mut out = format!("\", line {}, in {}\n", self.line, self.name);
        let Some(raw) = usize::try_from(self.line)
            .ok()
            .and_then(|line| line.checked_sub(1))
            .and_then(|i| match self.embedded_source {
                Some(text) => crate::objects::line_field(text, i),
                None => source?.lines().nth(i),
            })
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

/// Capture the bindings that FrameSummary will immediately convert to representations.
pub(super) fn frame_locals(ctx: &mut Context, frame: Id) -> Result<Id> {
    match *ctx.heap.get(frame) {
        Value::PythonFrame {
            unoptimized_locals: Some(namespace),
            ..
        } => Ok(namespace),
        Value::PythonFrame { scope: NONE, .. } => Ok(ctx.dict(Vec::new())),
        Value::PythonFrame { scope, code, .. } => ctx.locals_for_code(scope, code),
        _ => ctx.lookup_attr(frame, "f_locals"),
    }
}

/// Return the recorded line and expression byte range of a native traceback node.
pub(super) fn tb_position(ctx: &mut Context, tb: Id) -> Id {
    let Value::Traceback { lineno, span, .. } = *ctx.heap.get(tb) else {
        return NONE;
    };
    let line = ctx.int(lineno);
    let positions = match span {
        Some([start, _, end]) => vec![
            line,
            line,
            ctx.int(i64::from(start)),
            ctx.int(i64::from(end)),
        ],
        None => vec![line, NONE, NONE, NONE],
    };
    ctx.alloc(Value::Tuple(positions))
}

/// Visit the public traceback protocol, ignoring frames that cannot be cleared yet.
pub(super) fn clear_frames(ctx: &mut Context, mut tb: Id) -> Result<Id> {
    let runtime_error = ctx.builtin("RuntimeError");
    while tb != NONE {
        let attempt = (|| {
            let frame = ctx.lookup_attr(tb, "tb_frame")?;
            let clear = ctx.lookup_attr(frame, "clear")?;
            ctx.call(clear, Vec::new(), Vec::new())
        })();
        if let Err(err) = attempt
            && !ctx.raised_error_matches(&err, runtime_error)
        {
            return Err(err);
        }
        tb = ctx.lookup_attr(tb, "tb_next")?;
    }
    Ok(NONE)
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

/// Find the exception that precedes this one in its displayed traceback.
pub(in crate::dynamic) fn chain_link(error: &Error) -> Option<(Id, &'static str)> {
    match error.trace.as_ref().and_then(|trace| trace.cause) {
        Some(NONE) => None,
        Some(cause) => Some((
            cause,
            "\nThe above exception was the direct cause of the following exception:\n\n",
        )),
        None if error.context != NONE => Some((
            error.context,
            "\nDuring handling of the above exception, another exception occurred:\n\n",
        )),
        None => None,
    }
}

/// Render an uncaught chain before shutdown releases its Python exception objects.
pub(in crate::dynamic) fn report_chain(ctx: &mut Context, error: Error) {
    let exception = ctx.raised_instance(error);
    display(ctx, exception);
}

/// Write the Python exception to the live stderr, retaining the low-level fallback.
pub(in crate::dynamic) fn display(ctx: &mut Context, exception: Id) {
    use std::io::Write;

    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(exception);
    if print_uncaught(ctx, exception).is_err() {
        // stderr itself may be gone or unwritable. Keep the exception's arguments
        // visible on fd 2 without calling its Python string hook a second time.
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "object type name: {}", ctx.type_name(exception));
        let _ = writeln!(stderr, "object repr     : {}", ctx.repr(exception));
        let _ = writeln!(stderr, "lost sys.stderr");
    }
    ctx.heap.release_temporaries_since(mark);
}

fn print_uncaught(ctx: &mut Context, exception: Id) -> Result<Id> {
    let file = ctx
        .sys_binding("stderr")?
        .ok_or_else(|| Error::new_static("RuntimeError", "lost sys.stderr"))?;
    if file == NONE {
        return Ok(NONE);
    }
    ctx.heap.keep(file);
    // The interpreter selects the innermost frames for a positive limit.
    let limit = ctx
        .sys_binding("tracebacklimit")?
        .and_then(|id| ctx.as_int(id).ok())
        .map_or(i64::MAX, |n| {
            n.to_i64().unwrap_or_else(|| {
                if n.big().sign() == num_bigint::Sign::Minus {
                    0
                } else {
                    i64::MAX
                }
            })
        });
    let options = [
        ("file".to_owned(), file),
        ("limit".to_owned(), ctx.int(-limit.max(0))),
    ];
    print(ctx, &[exception], &options)
}

fn render(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    chain_index: usize,
) -> Result<Vec<TraceText<'static>>> {
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
    let limit = traceback_limit(ctx, option(args, 3, kwargs, "limit")?)?;
    let chain = option(args, chain_index, kwargs, "chain")?
        .map(|v| ctx.truth(v))
        .transpose()?
        .unwrap_or(true);
    render_chain(ctx, value, tb, limit, chain, None)
}

fn traceback_limit(ctx: &mut Context, limit: Option<Id>) -> Result<Option<i64>> {
    Ok(match limit.filter(|&l| l != NONE) {
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
    })
}

/// Render one boxed TestResult report from its supplied exception tuple.
pub(super) fn format_result(
    ctx: &mut Context,
    kind: Id,
    value: Id,
    tb: Id,
    locals: Id,
) -> Result<Id> {
    let limit = traceback_limit(ctx, None)?;
    let lines = render_chain(ctx, value, tb, limit, true, Some((kind, locals)))?;
    let values = lines
        .into_iter()
        .map(|line| line.into_python(ctx))
        .collect();
    Ok(ctx.alloc(Value::List(values)))
}

fn render_chain(
    ctx: &mut Context,
    value: Id,
    tb: Id,
    limit: Option<i64>,
    chain: bool,
    report: Option<(Id, Id)>,
) -> Result<Vec<TraceText<'static>>> {
    let mut kind = report.map(|(kind, _)| kind);
    let mut sections = Vec::new();
    let mut seen = std::collections::HashSet::from([value]);
    let (mut current, mut current_tb) = (value, tb);
    let mut separator: Option<&str> = None;
    loop {
        // Exception text is captured from the outermost error inwards. Its hook can
        // change the cause, drop references, and run Python's garbage collector.
        ctx.heap.keep(current);
        let mut lines = Vec::new();
        if let Some((_, locals)) = report
            && locals != NONE
        {
            format_result_frames(ctx, current_tb, limit, locals, &mut lines)?;
        } else {
            format_frames(ctx, current_tb, limit, &mut lines)?;
        }
        format_exception_line(ctx, current, kind.take(), &mut lines)?;
        if let Some(separator) = separator {
            lines.push(TraceText::Scalar(separator.to_owned().into()));
        }
        sections.push(lines);
        let Value::Raised(error) = ctx.heap.get(current) else {
            break;
        };
        let Some((prior, message)) = chain_link(error) else {
            break;
        };
        if !seen.insert(prior) {
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
    let values = lines
        .into_iter()
        .map(|line| line.into_python(ctx))
        .collect();
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
        let line = line.into_python(ctx);
        ctx.call(write, vec![line], Vec::new())?;
        let write = ctx.lookup_attr(file, "write")?;
        let end = ctx.text("");
        ctx.call(write, vec![end], Vec::new())?;
    }
    Ok(NONE)
}

/// Append Python traceback frames without rendering the exception or its chain.
pub(in crate::dynamic) fn format_frames(
    ctx: &mut Context,
    mut tb: Id,
    limit: Option<i64>,
    lines: &mut Vec<TraceText<'static>>,
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
        stack.push((code, lineno, span, String::new()));
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
    captured_frames(ctx, &stack, lines)
}

/// Capture each selected frame's locals before advancing its mutable traceback link.
fn format_result_frames(
    ctx: &mut Context,
    mut tb: Id,
    limit: Option<i64>,
    locals: Id,
    lines: &mut Vec<TraceText<'static>>,
) -> Result<()> {
    // TestResult uses sys.tracebacklimit, which traceback_limit clamps to zero or above.
    debug_assert!(limit.is_none_or(|n| n >= 0));
    let limit = limit
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(usize::MAX);
    let mut captured = Vec::new();
    while tb != NONE && captured.len() < limit {
        let Value::Traceback {
            frame,
            lineno,
            span,
            ..
        } = *ctx.heap.get(tb)
        else {
            return Err(Error::new("TypeError", "expected a traceback"));
        };
        let Value::PythonFrame { code, .. } = *ctx.heap.get(frame) else {
            unreachable!()
        };
        ctx.heap.reserve_temporaries(1)?;
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(tb);
        let text = ctx.call(locals, vec![frame], Vec::new())?;
        let text = ctx.str_value(text)?.to_owned();
        // A repr can detach the rest of the stack or replace it and run gc.collect().
        let Value::Traceback { next, .. } = *ctx.heap.get(tb) else {
            unreachable!()
        };
        tb = next;
        ctx.heap.keep_call_result(mark, code);
        captured.push((code, lineno, span, text));
    }
    captured_frames(ctx, &captured, lines)
}

type CapturedFrame = (Id, i64, Option<[u32; 3]>, String);

fn captured_frames(
    ctx: &mut Context,
    captured: &[CapturedFrame],
    lines: &mut Vec<TraceText<'static>>,
) -> Result<()> {
    // Arbitrary compile() filenames can be printed even when they cannot name a file.
    let mut paths = HashMap::<Id, Option<PathBuf>>::new();
    for (code, ..) in captured {
        let filename = ctx.code_object(*code).filename;
        paths
            .entry(filename)
            .or_insert_with(|| ctx.import_path(filename).ok());
    }
    let stack = captured
        .iter()
        .map(|(code, line, span, locals)| {
            let path = paths[&ctx.code_object(*code).filename].as_deref();
            FrameLine::new(ctx, *code, *line, *span, locals, path)
        })
        .collect::<Result<Vec<_>>>()?;
    render_frame_lines(&stack, lines);
    Ok(())
}

fn render_frame_lines(stack: &[FrameLine<'_>], lines: &mut Vec<TraceText<'static>>) {
    if !stack.is_empty() {
        lines.push(TraceText::Scalar(
            "Traceback (most recent call last):\n".into(),
        ));
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
                let source = frame.embedded_source.or_else(|| {
                    sources
                        .entry(&frame.file)
                        .or_insert_with(|| std::fs::read_to_string(frame.path?).ok())
                        .as_deref()
                });
                let mut suffix = frame.formatted_suffix(source);
                suffix.push_str(frame.locals);
                lines.push(frame.file.surrounded("  File \"", &suffix));
            }
            if same > 3 {
                let count = same - 3;
                lines.push(TraceText::Scalar(
                    format!(
                        "  [Previous line repeated {count} more time{}]\n",
                        if count == 1 { "" } else { "s" }
                    )
                    .into(),
                ));
            }
            i += same;
        }
    }
}

fn format_exception_line(
    ctx: &mut Context,
    value: Id,
    kind: Option<Id>,
    lines: &mut Vec<TraceText<'static>>,
) -> Result<()> {
    let convert = ctx.builtin("str");
    let text = match ctx.call(convert, vec![value], Vec::new()) {
        Ok(result) => TraceText::read(ctx, result)?.owned(),
        Err(_) => TraceText::Scalar("<exception str() failed>".into()),
    };
    let (module, dot, name) = match kind {
        Some(NONE) => ("", "", "None"),
        Some(kind) => {
            let name = ctx.lookup_attr(kind, "__qualname__")?;
            ctx.heap.keep(name);
            let module = ctx.lookup_attr(kind, "__module__")?;
            label_parts(ctx, module, name)?
        }
        None => exception_label(ctx, value)?,
    };
    let separator = if matches!(&text, TraceText::Scalar(s) if s.is_empty()) {
        ""
    } else {
        ": "
    };
    lines.push(text.surrounded(&format!("{module}{dot}{name}{separator}"), "\n"));
    Ok(())
}

/// Capture the exception class name after its string hook, as traceback does.
fn exception_label(ctx: &mut Context, value: Id) -> Result<(&str, &str, &str)> {
    let class = match ctx.heap.get(value) {
        Value::Raised(error) => error.class(),
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
    label_parts(ctx, module, name)
}

fn label_parts(ctx: &mut Context, module: Id, name: Id) -> Result<(&str, &str, &str)> {
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
    let displayed = super::super::builtins::format_value(ctx, label, None)?;
    ctx.str_value(displayed)
}
