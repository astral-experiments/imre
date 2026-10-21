//! Direct Rust port of the named incremental planner, with two numeric domains.

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::io::{self, BufRead, BufWriter, IsTerminal, Write};
use std::process::ExitCode;

#[cfg(feature = "batch")]
#[path = "batch.rs"]
mod batch;

/// Run the framed-request entrypoint while sharing the original graph algorithm.
#[cfg(feature = "batch")]
pub fn batch_main() -> ExitCode {
    batch::main()
}

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
    dependencies: Vec<String>,
}

enum Detail<'a> {
    Name(&'a str),
    Duration(&'a Number),
}

struct GraphError<'a> {
    kind: &'static str,
    index: usize,
    detail: Detail<'a>,
}

struct Plan<'a> {
    tasks: &'a [Task],
    order: Vec<usize>,
    starts: Vec<Number>,
    finishes: Vec<Number>,
    changed_count: usize,
    makespan: Number,
    error: Option<GraphError<'a>>,
}

/// Match the Python runtime's pipe buffering and terminal newline visibility.
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

/// Python's separator-free split includes these four additional ASCII controls.
fn python_whitespace(value: char) -> bool {
    value.is_whitespace() || matches!(value, '\u{001c}'..='\u{001f}')
}

/// Accept ASCII decimal digits, one sign, and underscores only between digits.
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

/// Counts and IDs must fit addressable storage in both numeric modes.
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

/// The bounded reference rejects overflow rather than wrapping scheduled times.
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
                "rebuild finish exceeds signed 64-bit range",
            )
        })
    }
}

/// Remove LF only; the token splitter handles a retained CR from CRLF input.
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

fn read_count(reader: &mut impl BufRead) -> Result<usize> {
    let count = parse_number(&input(reader)?)?;
    if count < Number::from(0) {
        return Err(Error::new("AssertionError", "count must be nonnegative"));
    }
    as_index(&count).ok_or_else(|| Error::new("OverflowError", "count exceeds addressable memory"))
}

/// Read every row before graph validation, matching the package's error phases.
fn read_problem(reader: &mut impl BufRead) -> Result<(Vec<Task>, Vec<String>)> {
    let count = read_count(reader)?;
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
                "target line requires a name and duration",
            ));
        }
        let mut dependencies = Vec::new();
        for (position, word) in words.iter().enumerate() {
            if position >= 2 {
                dependencies.push((*word).to_owned());
            }
        }
        tasks.push(Task {
            name: words[0].to_owned(),
            duration: parse_number(words[1])?,
            dependencies,
        });
    }
    let changed_count = read_count(reader)?;
    let mut changes = Vec::new();
    for _ in 0..changed_count {
        let line = input(reader)?;
        let words: Vec<&str> = line
            .split(python_whitespace)
            .filter(|word| !word.is_empty())
            .collect();
        if words.len() != 1 {
            return Err(Error::new(
                "AssertionError",
                "change line requires one name",
            ));
        }
        changes.push(words[0].to_owned());
    }
    Ok((tasks, changes))
}

/// Use the same sift-up operation as the Python source.
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

/// Vec's addressable capacity bounds make the child-index arithmetic fit usize.
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

fn failure<'a>(
    tasks: &'a [Task],
    kind: &'static str,
    index: usize,
    detail: Detail<'a>,
) -> Plan<'a> {
    Plan {
        tasks,
        order: Vec::new(),
        starts: Vec::new(),
        finishes: Vec::new(),
        changed_count: 0,
        makespan: Number::from(0),
        error: Some(GraphError {
            kind,
            index,
            detail,
        }),
    }
}

/// Preserve name-resolution phases, deduplication, whole-graph validation, set
/// closure, and both handwritten heap passes. Hash order never controls output.
// The shared Number alias needs owned clones in BigInt mode and copies in i64 mode.
#[cfg_attr(feature = "checked-i64", allow(clippy::clone_on_copy))]
fn schedule<'a>(tasks: &'a [Task], changes: &'a [String]) -> Result<Plan<'a>> {
    let count = tasks.len();
    let mut names: HashMap<&str, usize> = HashMap::new();
    for (task_id, task) in tasks.iter().enumerate() {
        if names.contains_key(task.name.as_str()) {
            return Ok(failure(
                tasks,
                "duplicate-name",
                task_id,
                Detail::Name(&task.name),
            ));
        }
        names.insert(&task.name, task_id);
    }
    // Python dict.items() follows insertion order; task order is the same order.
    for (task_id, task) in tasks.iter().enumerate() {
        if task.duration < Number::from(0) {
            return Ok(failure(
                tasks,
                "negative-duration",
                task_id,
                Detail::Duration(&task.duration),
            ));
        }
    }
    let mut dependencies: Vec<Vec<usize>> = Vec::new();
    let mut successors: Vec<Vec<usize>> = Vec::new();
    for _ in tasks {
        dependencies.push(Vec::new());
        successors.push(Vec::new());
    }
    let mut indegrees = vec![0_usize; count];
    for (task_id, task) in tasks.iter().enumerate() {
        let mut seen: HashSet<&str> = HashSet::new();
        for name in &task.dependencies {
            if !seen.contains(name.as_str()) {
                let Some(&dependency) = names.get(name.as_str()) else {
                    return Ok(failure(
                        tasks,
                        "unknown-dependency",
                        task_id,
                        Detail::Name(name),
                    ));
                };
                seen.insert(name);
                dependencies[task_id].push(dependency);
                successors[dependency].push(task_id);
                indegrees[task_id] += 1;
            }
        }
    }
    let mut changed = HashSet::new();
    for (change_id, name) in changes.iter().enumerate() {
        let Some(&task_id) = names.get(name.as_str()) else {
            return Ok(failure(
                tasks,
                "unknown-change",
                change_id,
                Detail::Name(name),
            ));
        };
        changed.insert(task_id);
    }
    let mut ready = Vec::new();
    for (task_id, &degree) in indegrees.iter().enumerate() {
        if degree == 0 {
            push_ready(&mut ready, task_id);
        }
    }
    let mut completed = 0;
    while !ready.is_empty() {
        let task_id = pop_ready(&mut ready);
        completed += 1;
        for &child in &successors[task_id] {
            indegrees[child] -= 1;
            if indegrees[child] == 0 {
                push_ready(&mut ready, child);
            }
        }
    }
    if completed != count {
        for (task_id, &degree) in indegrees.iter().enumerate() {
            if degree > 0 {
                return Ok(failure(
                    tasks,
                    "cycle",
                    task_id,
                    Detail::Name(&tasks[task_id].name),
                ));
            }
        }
    }

    let mut affected = HashSet::new();
    let mut queue = Vec::new();
    for &task_id in &changed {
        affected.insert(task_id);
        queue.push(task_id);
    }
    let mut cursor = 0;
    while cursor < queue.len() {
        let task_id = queue[cursor];
        for &child in &successors[task_id] {
            if !affected.contains(&child) {
                affected.insert(child);
                queue.push(child);
            }
        }
        cursor += 1;
    }
    let mut starts = vec![Number::from(0); count];
    let mut finishes = vec![Number::from(0); count];
    let mut remaining = vec![0_usize; count];
    for task_id in 0..count {
        if affected.contains(&task_id) {
            for dependency in &dependencies[task_id] {
                if affected.contains(dependency) {
                    remaining[task_id] += 1;
                }
            }
            if remaining[task_id] == 0 {
                push_ready(&mut ready, task_id);
            }
        }
    }
    let mut order = Vec::new();
    let mut makespan = Number::from(0);
    while !ready.is_empty() {
        let task_id = pop_ready(&mut ready);
        order.push(task_id);
        finishes[task_id] = add(&starts[task_id], &tasks[task_id].duration)?;
        if finishes[task_id] > makespan {
            makespan = finishes[task_id].clone();
        }
        for &child in &successors[task_id] {
            if finishes[task_id] > starts[child] {
                starts[child] = finishes[task_id].clone();
            }
            remaining[child] -= 1;
            if remaining[child] == 0 {
                push_ready(&mut ready, child);
            }
        }
    }
    assert_eq!(order.len(), affected.len());
    Ok(Plan {
        tasks,
        order,
        starts,
        finishes,
        changed_count: changed.len(),
        makespan,
        error: None,
    })
}

/// Emit exactly the package's report, including error details and every time.
// Borrow the shared Number alias so BigInt values stay owned by the plan.
#[cfg_attr(
    feature = "checked-i64",
    allow(clippy::needless_borrows_for_generic_args)
)]
fn show(plan: &Plan<'_>, output: &mut Output<impl Write>) -> Result<()> {
    if let Some(error) = &plan.error {
        output.line("ERROR")?;
        output.line(error.kind)?;
        output.line(error.index)?;
        match error.detail {
            Detail::Name(value) => output.line(value)?,
            Detail::Duration(value) => output.line(value)?,
        }
        return Ok(());
    }
    output.line("OK")?;
    output.line("CHANGED")?;
    output.line(plan.changed_count)?;
    output.line("AFFECTED")?;
    output.line(plan.order.len())?;
    output.line("PLAN")?;
    output.line(plan.order.len())?;
    for &task_id in &plan.order {
        output.line(&plan.tasks[task_id].name)?;
        output.line(task_id)?;
        output.line(&plan.tasks[task_id].duration)?;
        output.line(&plan.starts[task_id])?;
        output.line(&plan.finishes[task_id])?;
    }
    output.line("MAKESPAN")?;
    output.line(&plan.makespan)
}

fn run(reader: &mut impl BufRead, output: &mut Output<impl Write>) -> Result<()> {
    let (tasks, changes) = read_problem(reader)?;
    let plan = schedule(&tasks, &changes)?;
    show(&plan, output)?;
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
            let _ = output.flush();
            let _ = writeln!(io::stderr().lock(), "{}: {}", error.kind, error.message);
            ExitCode::FAILURE
        }
    }
}
