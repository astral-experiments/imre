//! Format native exception objects and write their tracebacks to Python streams.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use icu_properties::{CodePointMapData, props::EastAsianWidth};

use crate::dynamic::value::{TracebackAnchor, TracebackSpan};

use super::{Context, Error, Id, NONE, Result, Value};

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
    file: TraceText<'a>,
    path: Option<&'a Path>,
    embedded_source: Option<&'a str>,
    name: Option<&'a str>,
    line: i128,
    span: Option<TracebackSpan>,
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
        span: Option<TracebackSpan>,
        path: Option<&'a Path>,
    ) -> Result<Self> {
        let code = ctx.code_object(code);
        let file = TraceText::read(ctx, code.filename)?;
        // Only native code has the original embedded source at this position.
        let embedded_source = if !matches!(
            code.body,
            crate::dynamic::value::Body::Native(..) | crate::dynamic::value::Body::Suspended(_)
        ) {
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
            file,
            path,
            embedded_source,
            name: Some(ctx.str_value(code.name)?),
            line: i128::from(line),
            span,
        })
    }

    fn formatted_suffix(&self, source: Option<&str>) -> String {
        let mut out = format!("\", line {}", self.line);
        if let Some(name) = self.name {
            let _ = write!(out, ", in {name}");
        }
        out.push('\n');
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
        if let Some([start, split, caret_end, end]) = self
            .span
            .filter(|s| i128::from(s.end_line) == self.line)
            .and_then(|s| {
                let end = s.end as usize;
                let (split, caret_end) = match s.anchor {
                    TracebackAnchor::Uniform => (s.start as usize, end),
                    TracebackAnchor::Column(column) => (column as usize, end),
                    TracebackAnchor::Operator(column) => (column as usize, column as usize + 1),
                    TracebackAnchor::Hidden => return None,
                };
                Some([s.start as usize, split, caret_end, end])
            })
        {
            let indent = raw.len() - raw.trim_start().len();
            if let (Some(prefix), Some(primary), Some(secondary), Some(trailing)) = (
                raw.get(indent..start),
                raw.get(start..split),
                raw.get(split..caret_end),
                raw.get(caret_end..end),
            ) && (!primary.is_empty()
                || !prefix.trim().is_empty()
                || !raw[end..].trim().is_empty())
            {
                let _ = writeln!(
                    out,
                    "    {}{}{}{}",
                    " ".repeat(display_width(prefix)),
                    "~".repeat(display_width(primary)),
                    "^".repeat(display_width(secondary)),
                    "~".repeat(display_width(trailing))
                );
            }
        }
        out
    }
}

/// Return the recorded line and expression byte range of a native traceback node.
pub(super) fn tb_position(ctx: &mut Context, tb: Id) -> Id {
    let Value::Traceback { lineno, span, .. } = *ctx.heap.get(tb) else {
        return NONE;
    };
    let line = ctx.int(lineno);
    let positions = match span {
        Some(TracebackSpan {
            end_line,
            start,
            end,
            ..
        }) => vec![
            line,
            ctx.int(i64::from(end_line)),
            ctx.int(i64::from(start)),
            ctx.int(i64::from(end)),
        ],
        None => vec![line, NONE, NONE, NONE],
    };
    ctx.alloc(Value::Tuple(positions))
}

/// Find the exception that precedes this one in its displayed traceback.
pub(in crate::dynamic) fn chain_link(error: &Error) -> Option<(Id, &'static str)> {
    match error.trace.as_ref().and_then(|trace| trace.cause) {
        Some(cause) if cause != NONE => Some((
            cause,
            "\nThe above exception was the direct cause of the following exception:\n\n",
        )),
        _ if error.context != NONE && !error.suppresses_context() => Some((
            error.context,
            "\nDuring handling of the above exception, another exception occurred:\n\n",
        )),
        _ => None,
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
    let tb = if exception == NONE {
        NONE
    } else {
        ctx.exception_traceback(exception)?
    };
    let lines = render_chain(ctx, exception, tb, -limit.max(0))?;
    // A write callback can replace sys.stderr and trigger collection before the next line.
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

fn render_chain(
    ctx: &mut Context,
    value: Id,
    tb: Id,
    limit: i64,
) -> Result<Vec<TraceText<'static>>> {
    let mut sections = Vec::new();
    let mut seen = std::collections::HashSet::from([value]);
    let (mut current, mut current_tb) = (value, tb);
    let mut separator: Option<&str> = None;
    loop {
        // Exception text is captured from the outermost error inwards. Its hook can
        // change the cause, drop references, and run Python's garbage collector.
        ctx.heap.keep(current);
        let mut lines = Vec::new();
        format_frames(ctx, current_tb, Some(limit), &mut lines)?;
        format_exception_line(ctx, current, &mut lines)?;
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
    Ok(sections.into_iter().rev().flatten().collect())
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
        stack.push((code, lineno, span));
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

type CapturedFrame = (Id, i64, Option<TracebackSpan>);

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
        .map(|(code, line, span)| {
            let path = paths[&ctx.code_object(*code).filename].as_deref();
            FrameLine::new(ctx, *code, *line, *span, path)
        })
        .collect::<Result<Vec<_>>>()?;
    render_frame_lines(stack.into_iter(), |line| lines.push(line));
    Ok(())
}

/// Native raises and calls retain their checked source sites on the escaping exception.
pub(crate) fn write_raise_origins(
    origins: &[crate::objects::exceptions::RaiseOrigin],
    out: &mut impl std::io::Write,
) {
    use crate::objects::exceptions::RaiseOrigin;
    let frames = origins.iter().rev().map(|origin| {
        let (file, name, line, span) = match origin {
            RaiseOrigin::Line(file, line) => (file.as_str(), None, i128::from(*line), None),
            RaiseOrigin::Frame {
                file,
                name,
                line,
                span,
            } => (
                *file,
                Some(*name),
                i128::from(*line),
                span.map(|[start, split, end]| TracebackSpan {
                    end_line: *line,
                    start,
                    anchor: TracebackAnchor::Column(split),
                    end,
                }),
            ),
        };
        FrameLine {
            file: TraceText::Scalar(Cow::Borrowed(file)),
            path: (!(file.starts_with('<') && file.ends_with('>'))).then(|| Path::new(file)),
            embedded_source: None,
            name,
            line,
            span,
        }
    });
    render_frame_lines(frames, |line| {
        let TraceText::Scalar(text) = line else {
            unreachable!("native origins have UTF-8 filenames")
        };
        let _ = out.write_all(text.as_bytes());
    });
}

fn render_frame_lines<'a>(
    frames: impl Iterator<Item = FrameLine<'a>>,
    mut emit: impl FnMut(TraceText<'static>),
) {
    let mut frames = frames.peekable();
    if frames.peek().is_none() {
        return;
    }
    emit(TraceText::Scalar(
        "Traceback (most recent call last):\n".into(),
    ));
    let mut sources = HashMap::new();
    while let Some(first) = frames.next() {
        let same = |next: &FrameLine<'_>| {
            first.name.is_some()
                && next.file == first.file
                && next.name == first.name
                && next.line == first.line
        };
        let mut render = |frame: &FrameLine<'a>| {
            let source = frame.embedded_source.or_else(|| {
                let path = frame.path?;
                sources
                    .entry(path)
                    .or_insert_with(|| std::fs::read_to_string(path).ok())
                    .as_deref()
            });
            emit(
                frame
                    .file
                    .surrounded("  File \"", &frame.formatted_suffix(source)),
            );
        };
        render(&first);
        let mut count = 1;
        while let Some(frame) = frames.next_if(same) {
            count += 1;
            if count <= 3 {
                render(&frame);
            }
        }
        if count > 3 {
            let repeated = count - 3;
            emit(TraceText::Scalar(
                format!(
                    "  [Previous line repeated {repeated} more time{}]\n",
                    if repeated == 1 { "" } else { "s" }
                )
                .into(),
            ));
        }
    }
}

fn format_exception_line(
    ctx: &mut Context,
    value: Id,
    lines: &mut Vec<TraceText<'static>>,
) -> Result<()> {
    let convert = ctx.builtin("str");
    let text = match ctx.call(convert, vec![value], Vec::new()) {
        Ok(result) => TraceText::read(ctx, result)?.owned(),
        Err(_) => TraceText::Scalar("<exception str() failed>".into()),
    };
    let (module, dot, name) = exception_label(ctx, value)?;
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
