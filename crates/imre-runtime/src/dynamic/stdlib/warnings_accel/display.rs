//! Source-loader lookup and warning display for the native warnings provider.

use std::io::Write;
use std::path::Path;

use crate::dynamic::stdlib::diagnostic_type_name;
use crate::dynamic::{Context, Error, Id, NONE, Result, Value};

/// Ask the interpreter's bootstrap loader for source before warning filters run.
pub(super) fn source_line(ctx: &mut Context, globals: Id, lineno: i32) -> Result<Option<Id>> {
    let external = ctx.lookup_attr(ctx.bootstrap_module, "_bootstrap_external")?;
    let bless = ctx.lookup_attr(external, "_bless_my_loader")?;
    let loader = ctx.call(bless, vec![globals], Vec::new())?;
    ctx.heap.keep(loader);

    let dict = ctx.dict_storage(globals).expect("validated module globals");
    let key = ctx.heap.code_name("__name__");
    let (_, pos) = ctx.find_dictionary_key(dict, key, None)?;
    let Some(pos) = pos else { return Ok(None) };
    let name = ctx.stored_entries(dict)[pos].1;
    ctx.heap.keep(name);
    let get = match ctx.lookup_attr(loader, "get_source") {
        Ok(get) => get,
        Err(e) if ctx.is_attribute_error(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    let source = ctx.call(get, vec![name], Vec::new())?;
    if source == NONE {
        return Ok(None);
    }
    // PyUnicode_Splitlines ignores overrides on str subclasses.
    let text = ctx.string_storage(source).ok_or_else(|| {
        let actual = diagnostic_type_name(ctx, source);
        Error::new("TypeError", format!("must be str, not {actual}"))
    })?;
    let points = ctx.string_codepoints(text)?;
    // Read the requested field directly as Python code points, including lone surrogates.
    let mut tail = points.as_ref();
    for number in 1..=lineno {
        if tail.is_empty() {
            break;
        }
        let end = tail
            .iter()
            .position(|p| matches!(*p, 0x0a..=0x0d | 0x1c..=0x1e | 0x85 | 0x2028 | 0x2029))
            .unwrap_or(tail.len());
        if number == lineno {
            if end == points.len() {
                ctx.heap.keep(text);
                return Ok(Some(text));
            }
            let line = tail[..end].to_vec();
            return Ok(Some(ctx.text_codepoints(line)));
        }
        let delimiter = &tail[end..];
        tail = delimiter
            .strip_prefix(&[0x0d, 0x0a])
            .unwrap_or_else(|| delimiter.get(1..).unwrap_or_default());
    }
    Err(Error::new("IndexError", "list index out of range"))
}

/// Dispatch through the active warnings module, including its public WarningMessage factory.
pub(super) fn show(
    ctx: &mut Context,
    text: Id,
    message: Id,
    category: Id,
    filename: Id,
    lineno: i32,
    source: Id,
    line: Option<Id>,
) -> Result<()> {
    let Some(show) = super::warnings_attr(ctx, "_showwarnmsg", true)? else {
        // The C fallback clears display errors (including failures in sys.stderr.write).
        let _ = fallback(ctx, text, category, filename, lineno, line);
        return Ok(());
    };
    ctx.heap.keep(show);
    if !ctx.callable(show) {
        return Err(Error::new(
            "TypeError",
            "warnings._showwarnmsg() must be set to a callable",
        ));
    }
    let factory = super::warnings_attr(ctx, "WarningMessage", false)?
        .ok_or_else(|| Error::new("RuntimeError", "unable to get warnings.WarningMessage"))?;
    let number = ctx.int(i64::from(lineno));
    let mut args = vec![message, category, filename, number, NONE, NONE];
    if source != NONE {
        args.push(source);
    }
    let msg = ctx.call(factory, args, Vec::new())?;
    ctx.call(show, vec![msg], Vec::new())?;
    Ok(())
}

/// Write each field separately. stderr.write may be replaced between calls.
fn write(ctx: &mut Context, file: Id, item: Id) -> Result<()> {
    let method = ctx.lookup_attr(file, "write")?;
    ctx.heap.keep(method);
    let str_type = ctx.builtin("str");
    let text = ctx.call(str_type, vec![item], Vec::new())?;
    ctx.call(method, vec![text], Vec::new())?;
    Ok(())
}

fn fallback(
    ctx: &mut Context,
    text: Id,
    category: Id,
    filename: Id,
    lineno: i32,
    line: Option<Id>,
) -> Result<()> {
    let name = ctx.lookup_attr(category, "__name__")?;
    ctx.heap.keep(name);
    let Ok(Some(file)) = ctx.sys_binding("stderr") else {
        let _ = writeln!(std::io::stderr().lock(), "lost sys.stderr");
        return Ok(());
    };
    ctx.heap.keep(file);
    let location = ctx.text(format!(":{lineno}: "));
    let separator = ctx.text(": ");
    let newline = ctx.text("\n");
    for field in [filename, location, name, separator, text, newline] {
        write(ctx, file, field)?;
    }
    if let Some(line) = line {
        write(ctx, file, line)?;
        write(ctx, file, newline)?;
    } else if let Some(line) = file_line(ctx, filename, lineno) {
        let indent = ctx.text("  ");
        write(ctx, file, indent)?;
        write(ctx, file, line)?;
        write(ctx, file, newline)?;
    }
    Ok(())
}

/// Search sys.path for the basename only after opening the diagnostic filename fails.
fn open_source(ctx: &mut Context, filename: Id, open: Id, mode: Id) -> Option<Id> {
    if let Ok(file) = ctx.call(open, vec![filename, mode], Vec::new()) {
        return Some(file);
    }
    let tail = Path::new(ctx.str_value(filename).ok()?)
        .file_name()?
        .to_owned();
    let path = ctx.sys_binding("path").ok()??;
    let list = ctx.list_storage(path)?;
    ctx.heap.keep(list);
    let Value::List(entries) = ctx.heap.get(list) else {
        return None;
    };
    for i in 0..entries.len() {
        // An open hook can replace entries after each unsuccessful attempt.
        let Value::List(entries) = ctx.heap.get(list) else {
            return None;
        };
        let dir = *entries.get(i)?;
        let Ok(dir) = ctx.str_value(dir) else {
            continue;
        };
        let path = Path::new(dir).join(&tail);
        let name = ctx.text(path.to_str()?);
        if let Ok(file) = ctx.call(open, vec![name, mode], Vec::new()) {
            return Some(file);
        }
    }
    None
}

fn file_line(ctx: &mut Context, filename: Id, lineno: i32) -> Option<Id> {
    let name = ctx.str_value(filename).ok()?;
    if name.starts_with('<') && name.ends_with('>') {
        return None;
    }
    let index = usize::try_from(i64::from(lineno) - 1).ok()?;
    let io = ctx.import_with_builtins("io").ok()?;
    let open = ctx.lookup_attr(io, "open").ok()?;
    ctx.heap.keep(open);
    let mode = ctx.text("rb");
    let file = open_source(ctx, filename, open, mode)?;
    ctx.heap.keep(file);
    let source = read_source(ctx, file, index);
    if let Ok(close) = ctx.lookup_attr(file, "close") {
        let _ = ctx.call(close, Vec::new(), Vec::new());
    }
    source.map(|text| ctx.text(text))
}

fn read_source(ctx: &mut Context, file: Id, index: usize) -> Option<String> {
    let fileno = ctx.lookup_attr(file, "fileno").ok()?;
    let fd = ctx.call(fileno, Vec::new(), Vec::new()).ok()?;
    if ctx.as_integer(fd).ok()? < 0 {
        return None;
    }
    let readline = ctx.lookup_attr(file, "readline").ok()?;
    ctx.heap.keep(readline);
    let mut bytes = Vec::new();
    // Include both potential encoding-cookie lines, even when displaying line 1.
    for _ in 0..(index + 1).max(2) {
        let mark = ctx.heap.temporary_mark();
        let value = ctx.call(readline, Vec::new(), Vec::new()).ok()?;
        let Value::Bytes(chunk) = ctx.heap.get(value) else {
            return None;
        };
        if chunk.is_empty() {
            break;
        }
        bytes.try_reserve(chunk.len()).ok()?;
        bytes.extend_from_slice(chunk);
        ctx.heap.release_temporaries_since(mark);
    }
    let lines = crate::objects::decode_source_lines(&bytes).ok()?;
    let text = lines.get(index)?.strip_suffix('\n')?;
    Some(text.trim_start_matches([' ', '\t', '\x0c']).to_owned())
}
