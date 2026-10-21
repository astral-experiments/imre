//! A direct Rust reference for examples/dependency_planner, not a Python runtime.

use std::fmt::Display;
use std::io::{self, BufRead, BufWriter, IsTerminal, Write};
use std::process::ExitCode;

#[cfg(all(feature = "bigint", feature = "checked-i64"))]
compile_error!("select exactly one numeric mode: bigint or checked-i64");
#[cfg(not(any(feature = "bigint", feature = "checked-i64")))]
compile_error!("select exactly one numeric mode: bigint or checked-i64");

#[cfg(feature = "bigint")]
use num_traits::ToPrimitive;
#[cfg(feature = "bigint")]
type Number = num_bigint::BigInt;
#[cfg(feature = "checked-i64")]
type Number = i64;

type Result<T> = std::result::Result<T, Error>;

struct Error {
    kind: &'static str,
    message: &'static str,
}

impl Error {
    fn new(kind: &'static str, message: &'static str) -> Self {
        Self { kind, message }
    }
}

struct Task {
    name: String,
    duration: Number,
    dependencies: Vec<Number>,
}

struct GraphError {
    kind: &'static str,
    task_id: usize,
    detail: Number,
}

struct Plan<'a> {
    tasks: &'a [Task],
    order: Vec<usize>,
    starts: Vec<Number>,
    finishes: Vec<Number>,
    critical_path: Vec<usize>,
    makespan: Number,
    error: Option<GraphError>,
}

impl Plan<'_> {
    /// Scans input order, so duplicate names return the first matching task.
    fn find(&self, name: &str) -> Option<&Task> {
        self.tasks.iter().find(|task| task.name == name)
    }
}

/// Buffers pipe/file output at 8 KiB and flushes each line for terminals.
struct Output<W: Write> {
    writer: BufWriter<W>,
    line_buffered: bool,
}

impl<W: Write> Output<W> {
    fn new(writer: W, line_buffered: bool) -> Self {
        Self {
            writer: BufWriter::with_capacity(8 * 1024, writer),
            line_buffered,
        }
    }

    fn line(&mut self, value: impl Display) -> Result<()> {
        writeln!(self.writer, "{value}")
            .map_err(|_| Error::new("OSError", "could not write standard output"))?;
        if self.line_buffered {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.writer
            .flush()
            .map_err(|_| Error::new("OSError", "could not write standard output"))
    }
}

/// Python's separator-free str.split also treats U+001C through U+001F as space.
fn python_whitespace(value: char) -> bool {
    value.is_whitespace() || matches!(value, '\u{001c}'..='\u{001f}')
}

/// Parses the example's ASCII decimal format: one optional sign and underscores
/// only between digits. Surrounding Unicode whitespace follows int(str), which
/// excludes the four extra ASCII separators accepted by str.split.
fn parse_number(source: &str) -> Result<Number> {
    let source = source.trim();
    let digits = source.strip_prefix(['+', '-']).unwrap_or(source);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits
            .bytes()
            .all(|value| value.is_ascii_digit() || value == b'_')
    {
        return Err(Error::new(
            "ValueError",
            "expected an ASCII decimal integer",
        ));
    }
    let normalized = source.replace('_', "");
    #[cfg(feature = "bigint")]
    {
        Number::parse_bytes(normalized.as_bytes(), 10)
            .ok_or_else(|| Error::new("ValueError", "invalid integer input"))
    }
    #[cfg(feature = "checked-i64")]
    {
        normalized
            .parse::<Number>()
            .map_err(|_| Error::new("OverflowError", "integer exceeds signed 64-bit range"))
    }
}

/// Converts only nonnegative, representable values; invalid dependencies stay
/// Numbers until validation so their complete original value can be reported.
fn as_index(value: &Number) -> Option<usize> {
    #[cfg(feature = "bigint")]
    {
        value.to_usize()
    }
    #[cfg(feature = "checked-i64")]
    {
        usize::try_from(*value).ok()
    }
}

/// The bounded variant rejects an overflowing scheduled finish instead of
/// changing the graph's times by wrapping or saturating.
fn add(left: &Number, right: &Number) -> Result<Number> {
    #[cfg(feature = "bigint")]
    {
        Ok(left + right)
    }
    #[cfg(feature = "checked-i64")]
    {
        left.checked_add(*right).ok_or_else(|| {
            Error::new(
                "OverflowError",
                "scheduled finish exceeds signed 64-bit range",
            )
        })
    }
}

/// Reads one UTF-8 input line, removing only LF and retaining a preceding CR.
/// EOF before any bytes is an input error; a final line need not end in LF.
fn input(reader: &mut impl BufRead) -> Result<String> {
    let mut line = String::new();
    let length = reader
        .read_line(&mut line)
        .map_err(|_| Error::new("OSError", "could not read standard input"))?;
    if length == 0 {
        return Err(Error::new("EOFError", "EOF when reading a line"));
    }
    if line.ends_with('\n') {
        line.pop();
    }
    Ok(line)
}

/// Reads all task rows and the lookup before validating graph semantics, as the
/// Python package does. Token slices borrow each line; stored names own a copy.
fn read_problem(reader: &mut impl BufRead) -> Result<(Vec<Task>, String)> {
    let count = parse_number(&input(reader)?)?;
    if count < Number::from(0) {
        return Err(Error::new(
            "AssertionError",
            "task count must be nonnegative",
        ));
    }
    let count = as_index(&count)
        .ok_or_else(|| Error::new("OverflowError", "task count exceeds addressable memory"))?;
    let mut tasks = Vec::new();
    for _ in 0..count {
        let line = input(reader)?;
        let words: Vec<&str> = line
            .split(python_whitespace)
            .filter(|word| !word.is_empty())
            .collect();
        if words.len() < 2 {
            return Err(Error::new(
                "AssertionError",
                "task line requires a name and duration",
            ));
        }
        let duration = parse_number(words[1])?;
        let mut dependencies = Vec::new();
        for word in &words[2..] {
            dependencies.push(parse_number(word)?);
        }
        tasks.push(Task {
            name: words[0].to_owned(),
            duration,
            dependencies,
        });
    }
    let line = input(reader)?;
    let words: Vec<&str> = line
        .split(python_whitespace)
        .filter(|word| !word.is_empty())
        .collect();
    if words.len() > 1 {
        return Err(Error::new(
            "AssertionError",
            "lookup must contain at most one name",
        ));
    }
    let lookup = words.first().copied().unwrap_or("").to_owned();
    Ok((tasks, lookup))
}

/// Inserts one task ID into an existing min-heap using the Python sift-up loop.
fn push_ready(ready: &mut Vec<usize>, task_id: usize) {
    ready.push(task_id);
    let mut position = ready.len() - 1;
    while position > 0 {
        let parent = (position - 1) / 2;
        if ready[parent] <= task_id {
            break;
        }
        ready[position] = ready[parent];
        position = parent;
    }
    ready[position] = task_id;
}

/// Removes the minimum from a nonempty heap using the Python sift-down loop.
/// Vec<usize>'s allocation bound makes the child-index arithmetic representable.
fn pop_ready(ready: &mut Vec<usize>) -> usize {
    let result = ready[0];
    let last = ready.pop().expect("pop_ready requires a nonempty heap");
    let count = ready.len();
    if count > 0 {
        let mut position = 0;
        while position * 2 + 1 < count {
            let mut child = position * 2 + 1;
            if child + 1 < count && ready[child + 1] < ready[child] {
                child += 1;
            }
            if ready[child] >= last {
                break;
            }
            ready[position] = ready[child];
            position = child;
        }
        ready[position] = last;
    }
    result
}

fn failure<'a>(tasks: &'a [Task], kind: &'static str, task_id: usize, detail: Number) -> Plan<'a> {
    Plan {
        tasks,
        order: Vec::new(),
        starts: Vec::new(),
        finishes: Vec::new(),
        critical_path: Vec::new(),
        makespan: Number::from(0),
        error: Some(GraphError {
            kind,
            task_id,
            detail,
        }),
    }
}

/// Mirrors the package's adjacency construction, min-heap schedule, and parent
/// traversal. Validation is in task/edge input order; duplicate edges remain
/// duplicate adjacency entries and indegree contributions. Ties select the
/// smallest ready task, predecessor, and terminal ID respectively.
fn schedule(tasks: &[Task]) -> Result<Plan<'_>> {
    let count = tasks.len();
    let mut successors: Vec<Vec<usize>> = Vec::new();
    let mut indegrees = vec![0_usize; count];
    let mut starts = vec![Number::from(0); count];
    let mut finishes = vec![Number::from(0); count];
    let mut parents = vec![None; count];
    for _ in 0..count {
        successors.push(Vec::new());
    }
    for task_id in 0..count {
        let task = &tasks[task_id];
        if task.duration < Number::from(0) {
            return Ok(failure(
                tasks,
                "negative-duration",
                task_id,
                task.duration.clone(),
            ));
        }
        for dependency in &task.dependencies {
            let Some(dependency_id) = as_index(dependency).filter(|&id| id < count) else {
                return Ok(failure(
                    tasks,
                    "missing-dependency",
                    task_id,
                    dependency.clone(),
                ));
            };
            successors[dependency_id].push(task_id);
            indegrees[task_id] += 1;
        }
    }

    let mut ready = Vec::new();
    let mut order = Vec::new();
    for task_id in 0..count {
        if indegrees[task_id] == 0 {
            push_ready(&mut ready, task_id);
        }
    }
    for _ in 0..count {
        if ready.is_empty() {
            break;
        }
        let task_id = pop_ready(&mut ready);
        order.push(task_id);
        finishes[task_id] = add(&starts[task_id], &tasks[task_id].duration)?;
        for &child in &successors[task_id] {
            if finishes[task_id] > starts[child] {
                starts[child] = finishes[task_id].clone();
                parents[child] = Some(task_id);
            } else if finishes[task_id] == starts[child]
                && parents[child].is_none_or(|parent| task_id < parent)
            {
                parents[child] = Some(task_id);
            }
            indegrees[child] -= 1;
            if indegrees[child] == 0 {
                push_ready(&mut ready, child);
            }
        }
    }
    if order.len() != count {
        let blocked = indegrees
            .iter()
            .position(|&indegree| indegree > 0)
            .expect("an incomplete schedule has an unscheduled task");
        return Ok(failure(tasks, "cycle", blocked, Number::from(-1)));
    }

    let mut makespan = Number::from(0);
    let mut end = None;
    for task_id in 0..count {
        if successors[task_id].is_empty() && (end.is_none() || finishes[task_id] > makespan) {
            makespan = finishes[task_id].clone();
            end = Some(task_id);
        }
    }
    let mut reversed_path = Vec::new();
    while let Some(task_id) = end {
        reversed_path.push(task_id);
        end = parents[task_id];
    }
    let mut critical_path = Vec::new();
    for position in (0..reversed_path.len()).rev() {
        critical_path.push(reversed_path[position]);
    }
    Ok(Plan {
        tasks,
        order,
        starts,
        finishes,
        critical_path,
        makespan,
        error: None,
    })
}

/// Produces a fresh string for each Python '+' operation. In particular, the
/// path loop must not append into its old buffer or replace the loop with join.
fn concatenate(left: &str, right: &str) -> String {
    [left, right].concat()
}

/// Writes every report line in Python order, including error details, critical
/// IDs, repeated path concatenations, and the first matching input task's data.
fn show(plan: &Plan<'_>, lookup_name: &str, output: &mut Output<impl Write>) -> Result<()> {
    if let Some(error) = &plan.error {
        output.line("ERROR")?;
        output.line(error.kind)?;
        output.line(error.task_id)?;
        output.line(&error.detail)?;
        return Ok(());
    }
    output.line("OK")?;
    output.line("PLAN")?;
    output.line(plan.order.len())?;
    for &task_id in &plan.order {
        let task = &plan.tasks[task_id];
        output.line(&task.name)?;
        output.line(task_id)?;
        output.line(&task.duration)?;
        output.line(&plan.starts[task_id])?;
        output.line(&plan.finishes[task_id])?;
    }
    output.line("MAKESPAN")?;
    output.line(&plan.makespan)?;
    output.line("CRITICAL")?;
    output.line(plan.critical_path.len())?;
    let mut path_names = String::new();
    for (position, &task_id) in plan.critical_path.iter().enumerate() {
        output.line(task_id)?;
        if position > 0 {
            path_names = concatenate(&path_names, " -> ");
        }
        path_names = concatenate(&path_names, &plan.tasks[task_id].name);
    }
    output.line("PATH")?;
    output.line(&path_names)?;
    output.line("LOOKUP")?;
    if let Some(found) = plan.find(lookup_name) {
        output.line("FOUND")?;
        output.line(&found.name)?;
        output.line(&found.duration)?;
    } else {
        output.line("MISSING")?;
        output.line(lookup_name)?;
    }
    Ok(())
}

fn run(reader: &mut impl BufRead, output: &mut Output<impl Write>) -> Result<()> {
    let (tasks, lookup) = read_problem(reader)?;
    let plan = schedule(&tasks)?;
    show(&plan, &lookup, output)?;
    output.flush()
}

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let line_buffered = stdout.is_terminal();
    let mut output = Output::new(stdout.lock(), line_buffered);
    match run(&mut stdin.lock(), &mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Preserve a primary error if flushing pending output also fails.
            let _ = output.flush();
            let _ = writeln!(io::stderr().lock(), "{}: {}", error.kind, error.message);
            ExitCode::FAILURE
        }
    }
}
