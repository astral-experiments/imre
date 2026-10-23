mod classification_benchmark;
mod live;
mod model;
mod parse;
mod report;

use std::collections::HashMap;
use std::fs::File;
use std::hint::black_box;
use std::io::Write;
use std::os::fd::IntoRawFd;
use std::path::{Path, PathBuf};

use model::{FileStats, Side, TestCase, compute_stats, group_diagnostics};
use parse::Inputs;

type Result<T> = std::result::Result<T, Error>;

struct Error {
    kind: &'static str,
    message: String,
}

impl Error {
    fn value(message: impl Into<String>) -> Self {
        Self {
            kind: "ValueError",
            message: message.into(),
        }
    }
    fn assertion(message: impl Into<String>) -> Self {
        Self {
            kind: "AssertionError",
            message: message.into(),
        }
    }
    fn key(message: impl Into<String>) -> Self {
        Self {
            kind: "KeyError",
            message: message.into(),
        }
    }
    fn io(error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => "FileNotFoundError",
            std::io::ErrorKind::PermissionDenied => "PermissionError",
            std::io::ErrorKind::AlreadyExists => "FileExistsError",
            std::io::ErrorKind::IsADirectory => "IsADirectoryError",
            std::io::ErrorKind::NotADirectory => "NotADirectoryError",
            _ => "OSError",
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

fn normalized_path(path: &str) -> Result<PathBuf> {
    if path.as_bytes().contains(&0) {
        return Err(Error::value("embedded null byte"));
    }
    let result: PathBuf = Path::new(path).components().collect();
    Ok(if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    })
}

fn read_text(path: &str) -> Result<String> {
    let bytes = std::fs::read(normalized_path(path)?).map_err(Error::io)?;
    let text = String::from_utf8(bytes).map_err(|error| Error {
        kind: "UnicodeDecodeError",
        message: error.to_string(),
    })?;
    if text.contains('\r') {
        Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Ok(text)
    }
}

fn write_text(path: &str, text: &str) -> Result<()> {
    let mut file = File::create(normalized_path(path)?).map_err(Error::io)?;
    let written = file.write_all(text.as_bytes()).and_then(|()| file.flush());
    let descriptor = file.into_raw_fd();
    unsafe extern "C" {
        fn close(descriptor: i32) -> i32;
    }
    // Close is explicit so close errors are observable, as with Path.write_text.
    if unsafe { close(descriptor) } != 0 {
        return Err(Error::io(std::io::Error::last_os_error()));
    }
    written.map_err(Error::io)
}

struct Options {
    manifest: String,
    old: String,
    new: String,
    format: String,
    force_summary_table: bool,
    output: Option<String>,
    compute: bool,
    rounds: u64,
}

fn options() -> Result<Options> {
    let arguments: Vec<String> = std::env::args().collect();
    let mut result = Options {
        manifest: String::new(),
        old: String::new(),
        new: String::new(),
        format: "github".into(),
        force_summary_table: false,
        output: None,
        compute: false,
        rounds: 1,
    };
    let mut index = 1;
    while index < arguments.len() {
        let flag = &arguments[index];
        if flag == "--force-summary-table" {
            result.force_summary_table = true;
            index += 1;
            continue;
        }
        if flag == "--compute" {
            result.compute = true;
            index += 1;
            continue;
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Error::value(format!("expected a value after {flag}")))?;
        match flag.as_str() {
            "--manifest" => result.manifest.clone_from(value),
            "--old" => result.old.clone_from(value),
            "--new" => result.new.clone_from(value),
            "--format" => result.format.clone_from(value),
            "--output" => result.output = Some(value.clone()),
            "--rounds" => {
                result.rounds = value
                    .parse()
                    .map_err(|_| Error::value("rounds must be nonnegative"))?
            }
            _ => return Err(Error::value(format!("unknown argument: {flag}"))),
        }
        index += 2;
    }
    if result.manifest.is_empty() || result.old.is_empty() || result.new.is_empty() {
        return Err(Error::value(
            "expected --manifest SUITE.json --old OLD.json --new NEW.json",
        ));
    }
    if result.format != "github" && result.format != "diff" {
        return Err(Error::value("format must be github or diff"));
    }
    Ok(result)
}

fn collect_file_stats<'a>(cases: &[TestCase], inputs: &'a Inputs) -> Result<Vec<FileStats<'a>>> {
    let mut by_path: HashMap<&str, Vec<&TestCase>> = inputs
        .sources
        .iter()
        .map(|source| (&*source.path, Vec::new()))
        .collect();
    for case in cases {
        by_path
            .get_mut(case.path())
            .ok_or_else(|| Error::key(case.path()))?
            .push(case);
    }
    Ok(inputs
        .sources
        .iter()
        .map(|source| {
            let cases = &by_path[&*source.path];
            FileStats {
                path: &source.path,
                old: compute_stats(cases.iter().copied(), Side::Old),
                new: compute_stats(cases.iter().copied(), Side::New),
            }
        })
        .collect())
}

fn render_report(inputs: &Inputs, options: &Options) -> Result<String> {
    let expected = parse::collect_expected(&inputs.sources)?;
    let old = parse::collect_diagnostics(&inputs.old, &inputs.paths)?;
    let new = parse::collect_diagnostics(&inputs.new, &inputs.paths)?;
    let grouped = group_diagnostics(old, new, expected);
    let files = collect_file_stats(&grouped, inputs)?;
    let base = format!(
        "https://github.com/python/typing/blob/{}/conformance/",
        inputs.revision
    );
    let sections = [
        report::summary(&grouped, &files, &base, options.force_summary_table)?,
        report::file_stats(&files, &base),
        report::test_cases(&grouped, &base, &options.format),
    ];
    Ok(sections
        .into_iter()
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n"))
}

fn run() -> Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    if let Some(mode @ ("--classify" | "--entries")) = arguments.get(1).map(String::as_str) {
        return classification_benchmark::run(&arguments[2..], mode == "--entries");
    }
    if arguments.get(1).map(String::as_str) == Some("--scan") {
        return parse::scanner_benchmark::run(&arguments[2..]);
    }
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "--suite" | "--old-ty" | "--new-ty"))
    {
        return live::run(&arguments);
    }
    let options = options()?;
    let inputs = parse::read_inputs(&options.manifest, &options.old, &options.new)?;
    if options.compute {
        let mut checksum = num_bigint::BigInt::from(0);
        let mut rendered = String::new();
        for _ in 0..options.rounds {
            rendered = render_report(black_box(&inputs), black_box(&options))?;
            checksum += black_box(&rendered).chars().count();
        }
        if let Some(output) = &options.output {
            write_text(output, &rendered)?;
        }
        writeln!(std::io::stdout().lock(), "{checksum}").map_err(Error::io)?;
    } else {
        let rendered = render_report(&inputs, &options)?;
        if let Some(output) = &options.output {
            write_text(output, &rendered)?;
        } else {
            writeln!(std::io::stdout().lock(), "{rendered}").map_err(Error::io)?;
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{}: {}", error.kind, error.message);
        std::process::exit(1);
    }
}
