//! Bounded payload parsing and request recovery for the batch example.

use super::*;

/// Normalize payload integer errors without hiding checked-i64 overflow.
fn integer(text: &str) -> Result<Number> {
    parse_number(text).map_err(|error| {
        if error.kind == "ValueError" {
            Error::new("ValueError", "invalid integer")
        } else {
            error
        }
    })
}

/// A huge count still parses earlier rows before reaching the bounded frame end.
fn count(text: &str, negative_message: &'static str) -> Result<usize> {
    let value = integer(text)?;
    if value < Number::from(0) {
        return Err(Error::new("ValueError", negative_message));
    }
    Ok(as_index(&value).unwrap_or(usize::MAX))
}

fn take<'a>(lines: &'a [String], position: &mut usize) -> Result<&'a str> {
    let line = lines
        .get(*position)
        .ok_or_else(|| Error::new("IndexError", "truncated payload"))?;
    *position += 1;
    Ok(line)
}

/// Preserve row parsing order and partial-allocation cleanup before recovery.
pub(super) fn parse(lines: &[String]) -> Result<(Vec<Task>, Vec<String>)> {
    let mut position = 0;
    let task_count = count(take(lines, &mut position)?, "negative target count")?;
    let mut tasks = Vec::new();
    for _ in 0..task_count {
        let words: Vec<&str> = take(lines, &mut position)?
            .split(python_whitespace)
            .filter(|word| !word.is_empty())
            .collect();
        if words.len() < 2 {
            return Err(Error::new(
                "AssertionError",
                "target row needs a name and duration",
            ));
        }
        let mut dependencies = Vec::new();
        for (index, word) in words.iter().enumerate() {
            if index >= 2 {
                dependencies.push((*word).to_owned());
            }
        }
        tasks.push(Task {
            name: words[0].to_owned(),
            duration: integer(words[1])?,
            dependencies,
        });
    }
    let change_count = count(take(lines, &mut position)?, "negative change count")?;
    let mut changes = Vec::new();
    for _ in 0..change_count {
        let words: Vec<&str> = take(lines, &mut position)?
            .split(python_whitespace)
            .filter(|word| !word.is_empty())
            .collect();
        if words.len() != 1 {
            return Err(Error::new(
                "AssertionError",
                "change row needs exactly one name",
            ));
        }
        changes.push(words[0].to_owned());
    }
    Ok((tasks, changes))
}

/// The buffer is cleared and END is emitted for both recovered and escaping errors.
fn process(
    lines: &mut Vec<String>,
    request_id: usize,
    output: &mut Output<impl Write>,
) -> Result<bool> {
    output.line("REQUEST")?;
    output.line(request_id)?;
    let result = (|| {
        let (tasks, changes) = match parse(lines) {
            Ok(problem) => problem,
            Err(error) if matches!(error.kind, "ValueError" | "IndexError" | "AssertionError") => {
                output.line("INVALID")?;
                output.line(error.kind)?;
                output.line(error.message)?;
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        show(&schedule(&tasks, &changes)?, output)?;
        Ok(true)
    })();
    lines.clear();
    output.line("END")?;
    output.line(request_id)?;
    result
}

fn run(reader: &mut impl BufRead, output: &mut Output<impl Write>) -> Result<()> {
    let request_count = read_count(reader)?;
    let mut completed = 0;
    let mut invalid = 0;
    let mut cleaned = 0;
    for request_id in 0..request_count {
        let line_count = read_count(reader)?;
        let mut lines = Vec::new();
        for _ in 0..line_count {
            lines.push(input(reader)?);
        }
        if process(&mut lines, request_id, output)? {
            completed += 1;
        } else {
            invalid += 1;
        }
        debug_assert!(lines.is_empty());
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

pub(super) fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut output = Output::new(stdout.lock(), stdout.is_terminal());
    match run(&mut stdin.lock(), &mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = output.flush();
            let _ = writeln!(io::stderr().lock(), "{}: {}", error.kind, error.message);
            ExitCode::FAILURE
        }
    }
}
