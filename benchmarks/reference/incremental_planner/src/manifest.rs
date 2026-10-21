//! Sequential file-backed requests with normalized input failures.

use super::*;
use std::fs;
use std::io::Read;
use std::os::fd::IntoRawFd;

unsafe extern "C" {
    fn close(fd: std::ffi::c_int) -> std::ffi::c_int;
}

/// Preserve terminal OS categories while payload callers normalize their text.
fn io_error(error: io::Error) -> Error {
    let kind = match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        _ => "OSError",
    };
    Error::new(kind, "file operation failed")
}

/// Consume the sole file owner and report close errors without retrying Linux close.
fn checked_close(file: fs::File) -> Result<()> {
    let descriptor = file.into_raw_fd();
    // SAFETY: into_raw_fd transfers this live descriptor's exclusive ownership.
    if unsafe { close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(io_error(io::Error::last_os_error()))
    }
}

/// Close after parsing, including errors; close failure overrides the pending result.
fn with_input<T>(path: &str, work: impl FnOnce(&mut fs::File) -> Result<T>) -> Result<T> {
    if path.as_bytes().contains(&0) {
        return Err(Error::new("ValueError", "invalid payload path"));
    }
    let mut file = fs::File::open(path).map_err(io_error)?;
    let result = match file.metadata().map_err(io_error) {
        Ok(metadata) if metadata.is_dir() => {
            Err(Error::new("IsADirectoryError", "file operation failed"))
        }
        Ok(_) => work(&mut file),
        Err(error) => Err(error),
    };
    checked_close(file)?;
    result
}

/// Strict UTF-8 decoding; splitlines subsequently handles universal newlines.
fn read_utf8(file: &mut fs::File) -> Result<String> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error)?;
    String::from_utf8(bytes).map_err(|_| Error::new("UnicodeDecodeError", "invalid UTF-8"))
}

/// Match Python's complete splitlines boundary set, including CRLF as one break.
fn splitlines(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((position, character)) = characters.next() {
        if matches!(
            character,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            result.push(text[start..position].to_owned());
            start = position + character.len_utf8();
            if character == '\r' && characters.peek().is_some_and(|(_, next)| *next == '\n') {
                characters.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        result.push(text[start..].to_owned());
    }
    result
}

fn load_problem(path: &str) -> Result<(Vec<Task>, Vec<String>)> {
    with_input(path, |file| batch::parse(&splitlines(&read_utf8(file)?)))
}

/// Writes occur outside recovery; a report error always escapes the request.
fn process(path: &str, request_id: usize, output: &mut Output<impl Write>) -> Result<bool> {
    output.line("REQUEST")?;
    output.line(request_id)?;
    output.flush()?;
    let completed = match load_problem(path) {
        Ok((tasks, changes)) => {
            show(&schedule(&tasks, &changes)?, output)?;
            true
        }
        Err(error) => {
            let (kind, message) = match error.kind {
                "UnicodeDecodeError" => ("UnicodeDecodeError", "invalid UTF-8"),
                "OSError" | "FileNotFoundError" | "PermissionError" | "IsADirectoryError"
                | "NotADirectoryError" => ("OSError", "cannot read payload"),
                "ValueError" | "IndexError" | "AssertionError" => (error.kind, error.message),
                _ => return Err(error),
            };
            output.line("INVALID")?;
            output.line(kind)?;
            output.line(message)?;
            false
        }
    };
    output.line("END")?;
    output.line(request_id)?;
    Ok(completed)
}

fn requests(paths: &[String], output: &mut Output<fs::File>) -> Result<()> {
    let mut completed = 0;
    let mut invalid = 0;
    let mut cleaned = 0;
    for (request_id, path) in paths.iter().enumerate() {
        if process(path, request_id, output)? {
            completed += 1;
        } else {
            invalid += 1;
        }
        cleaned += 1;
    }
    output.line("SUMMARY")?;
    output.line("COMPLETED")?;
    output.line(completed)?;
    output.line("INVALID")?;
    output.line(invalid)?;
    output.line("CLEANED")?;
    output.line(cleaned)?;
    output.flush()
}

/// Flush and close even after a request failure, matching the report's with exit.
fn close_output(mut output: Output<fs::File>) -> Result<()> {
    let flushed = output.flush();
    let (file, unwritten) = output.writer.into_parts();
    drop(unwritten);
    checked_close(file)?;
    flushed
}

fn run(arguments: &[String]) -> Result<()> {
    if arguments.len() != 3 {
        return Err(Error::new(
            "ValueError",
            "usage: manifest_planner MANIFEST REPORT",
        ));
    }
    let paths = with_input(&arguments[1], |file| Ok(splitlines(&read_utf8(file)?)))?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&arguments[2])
        .map_err(io_error)?;
    let mut output = Output::new(file, false);
    let result = requests(&paths, &mut output);
    close_output(output)?;
    result
}

pub(super) fn main() -> ExitCode {
    match run(&std::env::args().collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{}: {}", error.kind, error.message);
            ExitCode::FAILURE
        }
    }
}
